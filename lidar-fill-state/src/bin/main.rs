// Don't use Rust's standard library because ESP32 doesn't support it.
#![no_std]
// Don't use Rust's standard main entrypoint.
// The entrypoint is provided by #[esp_rtos::main].
#![no_main]

use core::fmt::{Debug, Display, Write as FmtWrite};
use core::ffi::c_void;
use core::mem::size_of;
use core::sync::atomic::{AtomicU8, Ordering};
use edge_http::{
    io::{
        server::{
            Connection,
            DefaultServer,
            Handler,
        }
    },
    Method,
};
use edge_nal::TcpBind;
use edge_nal_embassy::{
    Tcp,
    TcpBuffers,
};
use esp_radio::wifi::DisconnectReason;
use log::{debug, error, info, warn, trace};
use embassy_executor::Spawner;
use embassy_net::{
    Config as NetConfig,
    Runner,
    Stack,
    StackResources,
};
use embassy_time::{Duration, Timer};
use embedded_io_async::{Read, Write};
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Level, Output, OutputConfig},
    peripherals::{
        WIFI
    }, 
    rmt::{
        Channel,
        PulseCode,
        Rmt,
        Tx,
        TxChannelConfig,
        TxChannelCreator,
    },
    rng::Rng,
    time::Rate,
    timer::timg::TimerGroup,
    Blocking,
};
use esp_hal::i2c::master::I2c;
use esp_println as _;
use heapless::Vec;
use esp_radio::wifi::{
    AuthenticationMethodConfig,
    Config as WifiConfig,
    Interface,
    WifiController,
    sta::StationConfig,
};
use static_cell::StaticCell;
use littlefs_rust::{
    Config, Filesystem, OpenFlags, Storage,
};
use esp_storage::FlashStorage;
use embassy_sync::{
    blocking_mutex::raw::NoopRawMutex,
    mutex::Mutex,
};

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum SystemState {
    Starting = 0,
    Running = 1,
    WifiError = 2,
    SensorError = 3,
}
/// The index where the storage partition starts
fn storage_offset() -> u32 {
    u32::from_str_radix(
        env!("STORAGE_PARTITION_START_INDEX").trim_start_matches("0x"),
        16,
    )
    .unwrap()
}
/// The size of a storage block
const STORAGE_BLOCK_SIZE: u32 = 4 * 1_024;
/// The amount of blocks in the storage
const STORAGE_BLOCK_COUNT: u32 = 528;
/// The size of the cache for the file system
const FILE_SYSTEM_CACHE_SIZE: u32 = 4 * 1_024;
/// The ssid of the wlan to connect to
const WIFI_SSID: &str = env!("WIFI_SSID");
/// The wifi password to use
const WIFI_PASSWORD: &str = env!("WIFI_PASSWORD");
/// The html of the index page of the webserver
const HTTP_INDEX_HTML: &str = include_str!("../../../web/index.html");
/// The html of the upload page of the webserver
const HTTP_UPLOAD_HTML: &str = include_str!("../../../web/upload.html");
/// The base css of the web pages of the webserver
const HTTP_BASE_STYLE_CSS: &str = include_str!("../../../web/base_style.css");

/// The state of the system
/// 
/// Primarily used for the RGB led on the esp32
static SYSTEM_STATE: AtomicU8 = AtomicU8::new(SystemState::Starting as u8);
/// STATIC NETWORK MEMORY
///
/// embassy-net needs memory that lives for the entire lifetime
/// of the program.
///
/// StackResources<8> gives the network stack room for several
/// sockets.
static STACK_RESOURCES: StaticCell<StackResources<8>> =
    StaticCell::new();
static TCP_BUFFERS: StaticCell<TcpBuffers<1, 8192, 8192>> =
    StaticCell::new();

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp_println::println!("PANIC: {}", info);

    loop {
        core::hint::spin_loop();
    }
}

// Put the application metadata into the firmware in the format the ESP32 bootloader expects.
esp_bootloader_esp_idf::esp_app_desc!();

/// The main program
#[esp_rtos::main] // This macro sets up the ESP RTOS/Embassy runtime.
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger(log::LevelFilter::Trace);

    info!("Programm starting...");

    trace!("Configuring the ESP and it's peripherls...");
    // Use the default configuration, but run the CPU at its maximum clock speed.
    let config = esp_hal::Config::default()
        .with_cpu_clock(CpuClock::max());

    // initializes the ESP32 hardware and gives us access to its peripherals.
    let peripherals = esp_hal::init(config);
    let esp_hal::peripherals::Peripherals {
        GPIO8,
        GPIO1,
        WIFI,
        RMT,
        TIMG0,
        FROM_CPU_INTR0,
        FLASH,
        ..
    } = peripherals;

    // This creates a heap. The heap is used by things that require dynamic allocation.
    // Heap from the main ram (shared with on borad peripherals)
    esp_alloc::heap_allocator!(
        size: 100 * 1024
    );
    // Heap from the bootloader ram
    esp_alloc::heap_allocator!(
        #[esp_hal::ram(reclaimed)]
        size: 64 * 1024
    );

    // Start Embassy runtime with the ESP32's Timergroup 0.
    let timg0 = TimerGroup::new(TIMG0);
    // Start the Runtime and CPU interrupts
    esp_rtos::start(timg0.timer0, FROM_CPU_INTR0);

    // Initializes the ESP32-C6's RMT hardware. Use RMT at 80 MHz.
    let rmt = Rmt::new(
        RMT,
        Rate::from_mhz(80),
    ).unwrap()
    .into_async();

    // Configuring the RMT transmitter.
    // The devider of 1 means 80Mhz / 1.
    // So one RMT tick is 12.5ns.
    // When no data is transmitted the output should be low.
    let tx_config = TxChannelConfig::default()
        .with_clk_divider(1)
        .with_idle_output_level(Level::Low);

    // Configure RMT channel 0 as a transmitter and route its output to GPIO8.
    // RGB LED is connected to GPIO8.
    let mut channel = rmt
        .channel0
        .configure_tx(&tx_config)
        .unwrap()
        .with_pin(GPIO8);

    debug!("Spawning a task for the RGB-LED...");
    // Start the LED-Task
    spawner.spawn(
        led_task(channel)
            .expect("failed to create LED task")
    );

    trace!("Setting up the network and wifi components...");
    let network_components = setup_network(WIFI);

    // Reset the sensor's I2C interface and select I2C mode.
    let mut sensor_i2c_mode = Output::new(
        GPIO1,
        Level::High,
        OutputConfig::default(),
    );
    let delay = Delay::new();
    delay.delay_millis(1);
    sensor_i2c_mode.set_low();
    delay.delay_millis(1);

    let i2c = I2c::new(
        peripherals.I2C0,
        esp_hal::i2c::master::Config::default()
            .with_frequency(esp_hal::time::Rate::from_khz(100)),
    )
    .unwrap()
    .with_sda(peripherals.GPIO6)
    .with_scl(peripherals.GPIO7);

    debug!("Spawning VL53L8CX sensor task...");
    spawner.spawn(
        sensor_task(i2c)
            .expect("failed to spawn sensor task")
    );

    // START NETWORK TASK
    //
    // The runner MUST run continuously.
    //
    // Without this task, TCP/IP won't actually process packets.
    debug!("Spawning network runner task...");
    spawner.spawn(
        net_task(network_components.runner)
            .expect("failed to spawn network task")
    );

    debug!("Spawning a task for wifi managment...");
    spawner.spawn(
        wifi_task(network_components.wifi_controller)
            .expect("failed to spawn Wi-Fi task")
    );

    debug!("Waiting for ipv4 config from DHCP...");
    network_components.stack.wait_config_up().await;

    if let Some(config) = network_components.stack.config_v4() {
        trace!("Received IPv4 config from DHCP: IP = {}", config.address);
    }

    debug!("Spawning a task for the web-server...");
    let tcp_buffers =
        TCP_BUFFERS.init(
            TcpBuffers::new()
        );

    let filesystem = FILESYSTEM.init(
        Mutex::new(init_filesystem(FLASH))
    );

    spawner.spawn(
        web_server_task(
            network_components.stack,
            tcp_buffers,
            filesystem,
        )
        .expect("failed to spawn web server task")
    );

    loop {
        log_static_sizes();
        log_heap_usage();

        Timer::after(
            Duration::from_secs(300)
        ).await;
    }
}

/// Sets the wifi and network-stack up and returns it
fn setup_network<'a>(wifi: WIFI<'static>) -> NetworkComponents<'a> {
    // Configure the ESP32 as a Wi-Fi station.
    //
    // Station = the ESP32 connects to your existing router.
    let station_config = StationConfig::default()
    .with_ssid(WIFI_SSID.try_into().unwrap())
    .with_authentication(
        AuthenticationMethodConfig::Wpa2Personal(
            WIFI_PASSWORD.try_into().unwrap(),
        ),
    );

    let wifi_config = WifiConfig::Station(station_config);

    // Create Wi-Fi controller.
    let mut wifi_controller =
        WifiController::new(
            wifi,
            Default::default(),
        )
        .expect("failed to initialize Wi-Fi");

    wifi_controller.set_config(&wifi_config).unwrap();

    // Create the station network interface.
    let wifi_interface =
        Interface::station();

    // Ask the router for an IPv4 address using DHCP.
    let net_config =
        NetConfig::dhcpv4(Default::default());

    // Create a random seed for the network stack.
    let rng = Rng::new();

    let seed =
        ((rng.random() as u64) << 32)
        | (rng.random() as u64);


    // Create the network stack.
    //
    // This gives us:
    //
    //     stack
    //       -> used by our HTTP server
    //
    //     runner
    //       -> continuously processes network packets
    //
    let (stack, runner) =
        embassy_net::new(
            wifi_interface,
            net_config,
            STACK_RESOURCES.init(
                StackResources::new()
            ),
            seed,
        );

    NetworkComponents {
        runner: runner,
        stack: stack,
        wifi_controller: wifi_controller
    }
}

/// This Task controls the RGB-LED and shows the state of the programm
#[embassy_executor::task]
async fn led_task(
    mut channel: Channel<'static, esp_hal::Async, Tx>,
) {
    loop {
        let state = match SYSTEM_STATE.load(Ordering::Relaxed) {
            0 => SystemState::Starting,
            1 => SystemState::Running,
            2 => SystemState::WifiError,
            3 => SystemState::SensorError,
            _ => SystemState::Starting,
        };
        
        let led_sequence: Vec<LedFlash, 10> = match state {
            SystemState::WifiError => {
                [
                    LedFlash { r: 0x05, g: 0x0, b: 0x05, duration: Duration::from_millis(500) },
                    LedFlash { r: 0x05, g: 0x05, b: 0x0, duration: Duration::from_millis(500) }
                ].into()
            }

            SystemState::Running => {
                [
                    LedFlash { r: 0x0, g: 0x05, b: 0x00, duration: Duration::from_millis(500) },
                    LedFlash { r: 0x02, g: 0x05, b: 0x02, duration: Duration::from_millis(500) }
                ].into()
            }

            _ => {
                [
                    LedFlash { r: 0x05, g: 0x0, b: 0x00, duration: Duration::from_millis(1000) },
                    LedFlash { r: 0x05, g: 0x02, b: 0x0, duration: Duration::from_millis(1000) }
                ].into()
            }
        };

        show_led_sequence(
            &mut channel, 
            led_sequence
        ).await;
    }
}

/// This Task continously tries to connect connect to the Wifi.
/// On a disconnect it tries to reconnect.
#[embassy_executor::task]
async fn wifi_task(
    mut controller: WifiController<'static>) -> ! {

    loop {

        debug!("Trying to connect the the wifi-network ({})...", WIFI_SSID);
        match controller.connect_async().await {

            Ok(connect_info) => {
                let authmodestr = match connect_info.authmode {
                    esp_radio::wifi::AuthenticationMethod::Dpp => "Dpp",
                    esp_radio::wifi::AuthenticationMethod::None => "None",
                    esp_radio::wifi::AuthenticationMethod::Owe => "Owe",
                    esp_radio::wifi::AuthenticationMethod::WapiPersonal => "WapiPersonal",
                    esp_radio::wifi::AuthenticationMethod::Wep => "Wep",
                    esp_radio::wifi::AuthenticationMethod::Wpa => "Wpa",
                    esp_radio::wifi::AuthenticationMethod::Wpa2Enterprise => "Wpa2Enterprise",
                    esp_radio::wifi::AuthenticationMethod::Wpa2Personal => "Wpa2Personal",
                    esp_radio::wifi::AuthenticationMethod::Wpa2Wpa3Enterprise => "Wpa2Wpa3Enterprise",
                    esp_radio::wifi::AuthenticationMethod::Wpa2Wpa3Personal => "Wpa2Wpa3Personal",
                    esp_radio::wifi::AuthenticationMethod::Wpa3EntSuiteB192Bit => "Wpa3EntSuiteB192Bit",
                    esp_radio::wifi::AuthenticationMethod::Wpa3Enterprise => "Wpa3Enterprise",
                    esp_radio::wifi::AuthenticationMethod::Wpa3ExtPsk => "Wpa3ExtPsk",
                    esp_radio::wifi::AuthenticationMethod::Wpa3ExtPskMixed => "Wpa3ExtPskMixed",
                    esp_radio::wifi::AuthenticationMethod::Wpa3Personal => "Wpa3Personal",
                    esp_radio::wifi::AuthenticationMethod::WpaEnterprise => "WpaEnterprise",
                    esp_radio::wifi::AuthenticationMethod::WpaWpa2Personal => "WpaWpa2Personal",
                    _ => "Unknown"
                };

                // Successfully connected.
                //
                // The network stack can now communicate with
                // the router.
                info!(
                    "Successfully connected to the Wi-Fi network \
                     (SSID: {}; Authmode: {}). \
                     The network stack can now communicate \
                     with the router.",
                    connect_info.ssid.as_str(),
                    authmodestr
                );
                SYSTEM_STATE.store(
                    SystemState::Running as u8,
                    Ordering::Relaxed,
                );
            }

            Err(connection_error) => {
                // Connection failed.
                //
                // Wait before trying again.
                let reason = match connection_error {
                    esp_radio::wifi::ConnectionError::Failed(disconnected_info) =>
                        to_string(disconnected_info.reason),

                    esp_radio::wifi::ConnectionError::WifiError(wifi_error) => {
                        match wifi_error {
                            esp_radio::wifi::WifiError::InvalidArguments => "InvalidArguments",
                            esp_radio::wifi::WifiError::InvalidPassword => "InvalidPassword",
                            esp_radio::wifi::WifiError::InvalidSsid => "InvalidSsid",
                            esp_radio::wifi::WifiError::NotConnected => "NotConnected",
                            esp_radio::wifi::WifiError::Other => "Other",
                            esp_radio::wifi::WifiError::OutOfMemory => "OutOfMemory",
                            esp_radio::wifi::WifiError::Unsupported => "Unsupported",
                            _ => "Unknown"
                        }
                    }

                    _ => "Unknown"
                };
                warn!("Error while conneting to the wifi: {}", reason);
                SYSTEM_STATE.store(
                    SystemState::WifiError as u8,
                    Ordering::Relaxed,
                );

                let wait_duration = Duration::from_secs(5);
                trace!("Waiting {} before retry...", wait_duration);
                Timer::after(wait_duration).await;

                continue;
            }
        }

        trace!("Waiting until wifi-connection disappears to then try to reconnect...");
        let _ =
            controller
                .wait_for_disconnect_async()
                .await;

        let retry_timeout_duration = Duration::from_secs(2);
        trace!("Waiting {} before trying to reconnect to wifi...", retry_timeout_duration);
        Timer::after(retry_timeout_duration).await;
    }
}

/// This Task keeps the network-card running
#[embassy_executor::task]
async fn net_task(
    mut runner: Runner<'static, Interface>) -> ! {

    // This is the actual Embassy network packet pump.
    //
    // It must keep running for TCP/IP to work.
    runner.run().await
}

/// This Task keeps the web-server running (observing the port 80 and sending responses)
#[embassy_executor::task]
async fn web_server_task(
    stack: Stack<'static>,
    tcp_buffers: &'static TcpBuffers<1, 8192, 8192>,
    filesystem: &'static Mutex<
        NoopRawMutex,
        Filesystem<LittleFsStorage<'static>>,
    >,
) -> ! {
    let tcp = Tcp::new(stack, tcp_buffers);

    let acceptor = tcp.bind("0.0.0.0:80".parse().unwrap())
        .await
        .expect("failed to bind HTTP port 80");

    let mut server = DefaultServer::new();

    info!("HTTP server starting on port 80...");

    loop {
        trace!("Waiting for HTTP connections...");

        match server
            .run_with_socket_queue::<_, _, 1>(
                None,
                acceptor,
                HttpHandler { filesystem },
            )
            .await
        {
            Ok(()) => warn!("HTTP server stopped; restarting..."),
            Err(error) => {
                error!("HTTP server error: {:?}", error);
                Timer::after(Duration::from_millis(100)).await;
            }
        }
    }
}

// ============================================================
// HTTP HANDLER
// ============================================================

struct HttpHandler {
    filesystem: &'static Mutex<
        NoopRawMutex,
        Filesystem<LittleFsStorage<'static>>,
    >,
}

impl HttpHandler {
    async fn handle_get_root<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "text/html; charset=utf-8")],
        ).await?;

        conn.write_all(HTTP_INDEX_HTML.as_bytes()).await?;
        Ok(())
    }

    async fn handle_get_hello<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "text/plain")],
        ).await?;

        conn.write_all(b"Hello from the ESP32-C6!\n").await?;
        Ok(())
    }

    async fn handle_get_upload<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "text/html; charset=utf-8")],
        ).await?;

        conn.write_all(HTTP_UPLOAD_HTML.as_bytes()).await?;
        Ok(())
    }

    async fn handle_post_upload<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        info!("Receiving file upload...");

        let mut buffer = [0u8; 8192];
        let mut total_bytes = 0usize;

        let fs = self.filesystem.lock().await;
        let file = match fs.open(
            "Test.bin",
            OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNC,
        ) {
            Ok(file) => file,
            Err(error) => {
                error!("Could not open Test.bin: {:?}", error);
                conn.initiate_response(
                    500,
                    Some("Internal Server Error"),
                    &[("Content-Type", "text/plain")],
                ).await?;
                conn.write_all(b"Upload failed\n").await?;
                return Ok(());
            }
        };

        loop {
            let n = conn.read(&mut buffer).await?;

            if n == 0 {
                break;
            }

            if let Err(error) = file.write(&buffer[..n]) {
                error!("File write failed: {:?}", error);
                conn.initiate_response(
                    500,
                    Some("Internal Server Error"),
                    &[("Content-Type", "text/plain")],
                ).await?;
                conn.write_all(b"Upload failed\n").await?;
                return Ok(());
            }

            total_bytes += n;
        }

        info!("File upload complete: {} bytes", total_bytes);

        conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "text/plain")],
        ).await?;

        conn.write_all(b"Upload successful\n").await?;
        Ok(())
    }

    async fn handle_get_file<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        info!("Sending Test.bin...");

        let mut buffer = [0u8; 1024];

        let mut fs = self.filesystem.lock().await;

        let mut file = match fs.open("Test.bin", OpenFlags::READ) {
            Ok(file) => file,
            Err(error) => {
                error!("Could not open Test.bin: {:?}", error);
                conn.initiate_response(
                    404,
                    Some("Not Found"),
                    &[("Content-Type", "text/plain")],
                ).await?;
                conn.write_all(b"File not found\n").await?;
                return Ok(());
            }
        };

        let mut content_length = heapless::String::<10>::new();
        write!(content_length, "{}", file.size()).unwrap();

        conn.initiate_response(
            200,
            Some("OK"),
            &[
                ("Content-Type", "application/octet-stream"),
                ("Content-Length", content_length.as_str()),
                ("Content-Disposition", "attachment; filename=\"Test.bin\"")
            ],
        ).await?;

        loop {
            let n = match file.read(&mut buffer) {
                Ok(n) => n,
                Err(error) => {
                    error!("File read failed: {:?}", error);
                    return Ok(());
                }
            };

            if n == 0 {
                return Ok(());
            }

            conn.write_all(&buffer[..n as usize]).await?;
        }
    }

    /// Returns the base_style.css
    async fn handle_get_base_css<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "text/css")],
        ).await?;

        conn.write_all(HTTP_BASE_STYLE_CSS.as_bytes()).await?;
        Ok(())
    }

    async fn handle_not_found<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        conn.initiate_response(
            404,
            Some("Not Found"),
            &[("Content-Type", "text/plain")],
        ).await?;

        conn.write_all(b"404 Not Found\n").await?;
        Ok(())
    }
}

impl Handler for HttpHandler {
    type Error<E>
        = edge_http::io::Error<E>
    where
        E: Debug;

    async fn handle<T, const N: usize>(
        &self,
        _task_id: impl Display + Copy,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), Self::Error<T::Error>>
    where
        T: Read + Write,
    {
        let headers = conn.headers()?;

        trace!("Received web request: {:?} {}", headers.method, headers.path);

        match (headers.method, headers.path) {
            (Method::Get, "/") => self.handle_get_root(conn).await,
            (Method::Get, "/hello") => self.handle_get_hello(conn).await,
            (Method::Get, "/upload") => self.handle_get_upload(conn).await,
            (Method::Post, "/upload") => self.handle_post_upload(conn).await,
            (Method::Get, "/file") => self.handle_get_file(conn).await,
            (Method::Get, "/assets/base_style.css") => self.handle_get_base_css(conn).await,
            _ => self.handle_not_found(conn).await,
        }
    }
}

/// Generates the RMT data for a WS2812.
///
/// The LED expects:
///
///     G R B
///
/// rather than:
///
///     R G B
///
/// Each color consists of 8 bits:
///
///     8 + 8 + 8 = 24 bits
///
/// We therefore need:
///
///     24 PulseCodes for the color
///     1  PulseCode for the reset/latch
///
/// Total:
///
///     25 PulseCodes
fn ws2812_data(
    r: u8,
    g: u8,
    b: u8,
) -> [PulseCode; 25] {

    // WS2812 uses GRB ordering.
    let bytes = [
        g,
        r,
        b,
    ];


    // Allocate space for:
    //
    // 24 color bits
    // + 1 reset pulse
    //
    let mut data = [
        PulseCode::default();
        25
    ];


    let mut bit = 0;


    // --------------------------------------------------------
    // Encode the 24 color bits
    // --------------------------------------------------------

    while bit < 24 {

        // Determine which color byte we're currently in.
        //
        // 0..7   = G
        // 8..15  = R
        // 16..23 = B
        //
        let byte = bytes[bit / 8];


        // Select the individual bit.
        //
        // 0x80 = 10000000
        //
        // The mask moves from the most significant bit
        // to the least significant bit.
        //
        let mask = 0x80 >> (bit % 8);


        if byte & mask != 0 {

            // ------------------------------------------------
            // Bit = 1
            //
            // 80 MHz RMT:
            //
            // 64 ticks HIGH = 800 ns
            // 36 ticks LOW  = 450 ns
            // ------------------------------------------------

            data[bit] = PulseCode::new(
                Level::High,
                64,
                Level::Low,
                36,
            );

        } else {

            // ------------------------------------------------
            // Bit = 0
            //
            // 32 ticks HIGH = 400 ns
            // 68 ticks LOW  = 850 ns
            // ------------------------------------------------

            data[bit] = PulseCode::new(
                Level::High,
                32,
                Level::Low,
                68,
            );
        }


        bit += 1;
    }


    // --------------------------------------------------------
    // Reset / latch
    // --------------------------------------------------------
    //
    // The LED needs the data line LOW for at least ~50 us
    // before it applies the received color.
    //
    // 4000 RMT ticks × 12.5 ns
    //
    // = 50 us
    //

    data[24] = PulseCode::new(
        Level::Low,
        4000,
        Level::Low,
        0,
    );


    data
}

/// Sendet an die LED die übergebene Farb-Sequenz
async fn show_led_sequence(
    channel: &mut Channel<'static, esp_hal::Async, Tx>,
    led_flashes: Vec<LedFlash, 10>)
{
    for led_flash in led_flashes {
        let data = ws2812_data(led_flash.r, led_flash.g, led_flash.b);

        channel
            .transmit(&data)
            .await
            .unwrap();

        Timer::after(led_flash.duration).await;
    }
}

struct LedFlash {
    r: u8,
    g: u8,
    b: u8,
    duration: Duration
}

struct NetworkComponents<'a> {
    runner: Runner<'static, Interface>,
    wifi_controller: WifiController<'a>,
    stack: Stack<'a>
}

pub struct LittleFsStorage<'d> {
    flash: FlashStorage<'d>,
}

impl<'d> LittleFsStorage<'d> {
    pub fn new(flash: FlashStorage<'d>) -> Self {
        Self { flash }
    }

    fn address(block: u32, offset: u32) -> u32 {
        storage_offset() + block * STORAGE_BLOCK_SIZE + offset
    }
}

impl Storage for LittleFsStorage<'_> {
    fn read(
        &mut self,
        block: u32,
        offset: u32,
        buf: &mut [u8],
    ) -> Result<(), littlefs_rust::Error> {
        let address = Self::address(block, offset);

        let result = self.flash
            .read_nor(address, buf)
            .map_err(|e| {
                error!(
                    "LFS READ FAILED: block={} offset={} len={} address=0x{:08X} error={:?}",
                    block,
                    offset,
                    buf.len(),
                    address,
                    e
                );

                littlefs_rust::Error::Io
            });

        result
    }


    fn write(
        &mut self,
        block: u32,
        offset: u32,
        data: &[u8],
    ) -> Result<(), littlefs_rust::Error> {
        let address = Self::address(block, offset);

        self.flash
            .write_nor(address, data)
            .map_err(|_| {
                error!(
                    "LFS WRITE FAILED: block={} offset={} len={} address=0x{:08X}",
                    block,
                    offset,
                    data.len(),
                    address
                );

                littlefs_rust::Error::Io
            })?;

        Ok(())
    }

    fn erase(
        &mut self,
        block: u32,
    ) -> Result<(), littlefs_rust::Error> {
        let address = Self::address(block, 0);

        self.flash
            .erase(address, address + STORAGE_BLOCK_SIZE)
            .map_err(|_| {
                error!(
                    "LFS ERASE FAILED: block={} address=0x{:08X}",
                    block,
                    address
                );

                littlefs_rust::Error::Io
            })?;

        Ok(())
    }


    fn sync(&mut self) -> Result<(), littlefs_rust::Error> {
        Ok(())
    }
}

fn init_filesystem(
    flash: esp_hal::peripherals::FLASH<'static>,
) -> Filesystem<LittleFsStorage<'static>> {
    let storage = LittleFsStorage::new(
        FlashStorage::new(flash),
    );

    let mut config = Config::new(
        STORAGE_BLOCK_SIZE,
        STORAGE_BLOCK_COUNT,
    );
    config.cache_size = FILE_SYSTEM_CACHE_SIZE;

    match Filesystem::mount(storage, config) {
        Ok(filesystem) => {
            info!("LittleFS mounted successfully!");
            filesystem
        }

        Err((littlefs_rust::Error::Corrupt, mut storage)) => {
            warn!("LittleFS is corrupt or unformatted. Formatting...");

            let mut format_config = Config::new(
                STORAGE_BLOCK_SIZE,
                STORAGE_BLOCK_COUNT,
            );
            format_config.cache_size = FILE_SYSTEM_CACHE_SIZE;

            Filesystem::format(
                &mut storage,
                &format_config,
            )
            .expect("LittleFS format failed");

            Filesystem::mount(storage, format_config)
                .map_err(|(error, _)| error)
                .expect("LittleFS mount after format failed")
        }

        Err((error, _)) => {
            panic!("LittleFS mount failed: {:?}", error);
        }
    }
}

static FILESYSTEM: StaticCell<
    Mutex<NoopRawMutex, Filesystem<LittleFsStorage<'static>>>
> = StaticCell::new();

fn log_heap_usage() {
    let used = esp_alloc::HEAP.used();
    let free = esp_alloc::HEAP.free();
    let mut total = used + free;
    
    if total == 0 {
        total = 1;
    }

    info!(
        "Heap: used={} B, free={} B, total={} B ({}%)",
        used,
        free,
        total,
        used * 100 / total
    );
}

fn log_static_sizes() {
    info!(
        "Static sizes: StackResources={} B, TcpBuffers={} B, Filesystem={} B",
        size_of::<StackResources<4>>(),
        size_of::<TcpBuffers<1, 512, 512>>(),
        size_of::<Filesystem<LittleFsStorage<'static>>>(),
    );
}

fn to_string(enum_to_format: esp_radio::wifi::DisconnectReason) -> &'static str{
    return match enum_to_format {
        DisconnectReason::AccessPointInitiatedDisassociation => "AccessPointInitiatedDisassociation",
        DisconnectReason::AccessPointTsfReset => "AccessPointTsfReset",
        DisconnectReason::AkmpInvalid => "AkmpInvalid",
        DisconnectReason::AlterativeChannelOccupied => "AlterativeChannelOccupied",
        DisconnectReason::AssociationComebackTimeTooLong => "AssociationComebackTimeTooLong",
        DisconnectReason::AssociationFailed => "AssociationFailed",
        DisconnectReason::AssociationLeave => "AssociationLeave",
        DisconnectReason::AssociationNotAuthenticated => "AssociationNotAuthenticated",
        DisconnectReason::AssociationTooMany => "AssociationTooMany",
        DisconnectReason::AuthenticationExpired => "AuthenticationExpired",
        DisconnectReason::AuthenticationFailed => "AuthenticationFailed",
        DisconnectReason::AuthenticationLeave => "AuthenticationLeave",
        DisconnectReason::BadCipherOrAkm => "BadCipherOrAkm",
        DisconnectReason::BeaconTimeout => "BeaconTimeout",
        DisconnectReason::BssTransitionDisassociated => "BssTransitionDisassociated",
        DisconnectReason::CipherSuiteRejected => "CipherSuiteRejected",
        DisconnectReason::Class2FrameFromNonAuthenticatedStation => "Class2FrameFromNonAuthenticatedStation",
        DisconnectReason::Class3FrameFromNonAssociatedStation => "Class3FrameFromNonAssociatedStation",
        DisconnectReason::ConnectionFailed => "ConnectionFailed",
        DisconnectReason::DisassociatedDueToInactivity => "DisassociatedDueToInactivity",
        DisconnectReason::DisassociatedPowerCapabilityBad => "DisassociatedPowerCapabilityBad",
        DisconnectReason::DisassociatedUnsupportedChannel => "DisassociatedUnsupportedChannel",
        DisconnectReason::EndBlockAck => "EndBlockAck",
        DisconnectReason::ExceededTxOp => "ExceededTxOp",
        DisconnectReason::FourWayHandshakeTimeout => "FourWayHandshakeTimeout",
        DisconnectReason::GroupCipherInvalid => "GroupCipherInvalid",
        DisconnectReason::GroupKeyUpdateTimeout => "GroupKeyUpdateTimeout",
        DisconnectReason::IeIn4wayDiffers => "IeIn4wayDiffers",
        DisconnectReason::IeInvalid => "IeInvalid",
        DisconnectReason::InvalidFtActionFrameCount => "InvalidFtActionFrameCount",
        DisconnectReason::InvalidFte => "InvalidFte",
        DisconnectReason::InvalidMde => "InvalidMde",
        DisconnectReason::InvalidPmkid => "InvalidPmkid",
        DisconnectReason::InvalidRsnIeCapabilities => "InvalidRsnIeCapabilities",
        DisconnectReason::MicFailure => "MicFailure",
        DisconnectReason::MissingAcks => "MissingAcks",
        DisconnectReason::NoAccessPointFound => "NoAccessPointFound",
        DisconnectReason::NoAccessPointFoundInAuthmodeThreshold => "NoAccessPointFoundInAuthmodeThreshold",
        DisconnectReason::NoAccessPointFoundInRssiThreshold => "NoAccessPointFoundInRssiThreshold",
        DisconnectReason::NoAccessPointFoundWithCompatibleSecurity => "NoAccessPointFoundWithCompatibleSecurity",
        DisconnectReason::NoSspRoamingAgreement => "NoSspRoamingAgreement",
        DisconnectReason::NotAuthorizedThisLocation => "NotAuthorizedThisLocation",
        DisconnectReason::NotEnoughBandwidth => "NotEnoughBandwidth",
        DisconnectReason::PairwiseCipherInvalid => "PairwiseCipherInvalid",
        DisconnectReason::PeerInitiated => "PeerInitiated",
        DisconnectReason::SaQueryTimeout => "SaQueryTimeout",
        DisconnectReason::ServiceChangePercludesTs => "ServiceChangePercludesTs",
        DisconnectReason::SspRequestedDisassociation => "SspRequestedDisassociation",
        DisconnectReason::StationLeaving => "StationLeaving",
        DisconnectReason::TdlsPeerUnreachable => "TdlsPeerUnreachable",
        DisconnectReason::TdlsUnspecified => "TdlsUnspecified",
        DisconnectReason::TransmissionLinkEstablishmentFailed => "TransmissionLinkEstablishmentFailed",
        DisconnectReason::UnknownBlockAck => "UnknownBlockAck",
        DisconnectReason::UnspecifiedQos => "UnspecifiedQos",
        DisconnectReason::UnsupportedRsnIeVersion => "UnsupportedRsnIeVersion",
        DisconnectReason::_802_1xAuthenticationFailed => "_802_1xAuthenticationFailed",
        _ => "Unknown"
    }
}

// fn to_string(enum_to_format: esp_radio::wifi::sta::DisconnectReason) -> &'static str{
//     return match enum_to_format {
//         DisconnectReason::AccessPointInitiatedDisassociation => "AccessPointInitiatedDisassociation",
//         DisconnectReason::AccessPointTsfReset => "AccessPointTsfReset",
//         DisconnectReason::AkmpInvalid => "AkmpInvalid",
//         DisconnectReason::AlterativeChannelOccupied => "AlterativeChannelOccupied",
//         DisconnectReason::AssociationComebackTimeTooLong => "AssociationComebackTimeTooLong",
//         DisconnectReason::AssociationFailed => "AssociationFailed",
//         DisconnectReason::AssociationLeave => "AssociationLeave",
//         DisconnectReason::AssociationNotAuthenticated => "AssociationNotAuthenticated",
//         DisconnectReason::AssociationTooMany => "AssociationTooMany",
//         DisconnectReason::AuthenticationExpired => "AuthenticationExpired",
//         DisconnectReason::AuthenticationFailed => "AuthenticationFailed",
//         DisconnectReason::AuthenticationLeave => "AuthenticationLeave",
//         DisconnectReason::BadCipherOrAkm => "BadCipherOrAkm",
//         DisconnectReason::BeaconTimeout => "BeaconTimeout",
//         DisconnectReason::BssTransitionDisassociated => "BssTransitionDisassociated",
//         DisconnectReason::CipherSuiteRejected => "CipherSuiteRejected",
//         DisconnectReason::Class2FrameFromNonAuthenticatedStation => "Class2FrameFromNonAuthenticatedStation",
//         DisconnectReason::Class3FrameFromNonAssociatedStation => "Class3FrameFromNonAssociatedStation",
//         DisconnectReason::ConnectionFailed => "ConnectionFailed",
//         DisconnectReason::DisassociatedDueToInactivity => "DisassociatedDueToInactivity",
//         DisconnectReason::DisassociatedPowerCapabilityBad => "DisassociatedPowerCapabilityBad",
//         DisconnectReason::DisassociatedUnsupportedChannel => "DisassociatedUnsupportedChannel",
//         DisconnectReason::EndBlockAck => "EndBlockAck",
//         DisconnectReason::ExceededTxOp => "ExceededTxOp",
//         DisconnectReason::FourWayHandshakeTimeout => "FourWayHandshakeTimeout",
//         DisconnectReason::GroupCipherInvalid => "GroupCipherInvalid",
//         DisconnectReason::GroupKeyUpdateTimeout => "GroupKeyUpdateTimeout",
//         DisconnectReason::IeIn4wayDiffers => "IeIn4wayDiffers",
//         DisconnectReason::IeInvalid => "IeInvalid",
//         DisconnectReason::InvalidFtActionFrameCount => "InvalidFtActionFrameCount",
//         DisconnectReason::InvalidFte => "InvalidFte",
//         DisconnectReason::InvalidMde => "InvalidMde",
//         DisconnectReason::InvalidPmkid => "InvalidPmkid",
//         DisconnectReason::InvalidRsnIeCapabilities => "InvalidRsnIeCapabilities",
//         DisconnectReason::MicFailure => "MicFailure",
//         DisconnectReason::MissingAcks => "MissingAcks",
//         DisconnectReason::NoAccessPointFound => "NoAccessPointFound",
//         DisconnectReason::NoAccessPointFoundInAuthmodeThreshold => "NoAccessPointFoundInAuthmodeThreshold",
//         DisconnectReason::NoAccessPointFoundInRssiThreshold => "NoAccessPointFoundInRssiThreshold",
//         InvalidRsnIeCapabilities => "InvalidRsnIeCapabilities",
//         DisconnectReason::NoAccessPointFoundWithCompatibleSecurity => "NoAccessPointFoundWithCompatibleSecurity",
//         DisconnectReason::NoSspRoamingAgreement => "NoSspRoamingAgreement",
//         DisconnectReason::NotAuthorizedThisLocation => "NotAuthorizedThisLocation",
//         DisconnectReason::NotEnoughBandwidth => "NotEnoughBandwidth",
//         DisconnectReason::PairwiseCipherInvalid => "PairwiseCipherInvalid",
//         DisconnectReason::PeerInitiated => "PeerInitiated",
//         DisconnectReason::SaQueryTimeout => "SaQueryTimeout",
//         DisconnectReason::ServiceChangePercludesTs => "ServiceChangePercludesTs",
//         DisconnectReason::SspRequestedDisassociation => "SspRequestedDisassociation",
//         DisconnectReason::StationLeaving => "StationLeaving",
//         DisconnectReason::TdlsPeerUnreachable => "TdlsPeerUnreachable",
//         DisconnectReason::TdlsUnspecified => "TdlsUnspecified",
//         DisconnectReason::TransmissionLinkEstablishmentFailed => "TransmissionLinkEstablishmentFailed",
//         DisconnectReason::UnknownBlockAck => "UnknownBlockAck",
//         DisconnectReason::UnspecifiedQos => "UnspecifiedQos",
//         DisconnectReason::UnsupportedRsnIeVersion => "UnsupportedRsnIeVersion",
//         DisconnectReason::_802_1xAuthenticationFailed => "_802_1xAuthenticationFailed",
//         _ => "Unknown"
//     }
// }



// C ======================

const VL53L8CX_I2C_ADDRESS: u8 = 0x29;
const VL53L8CX_RESOLUTION_8X8: u8 = 64;
const VL53L8CX_RESOLUTION_8X8_VALUES: usize = 64;
const VL53L8CX_TEMPORARY_BUFFER_SIZE: usize = 1452;
const VL53L8CX_OFFSET_BUFFER_SIZE: usize = 488;
const VL53L8CX_XTALK_BUFFER_SIZE: usize = 776;

type Vl53Write = extern "C" fn(*mut c_void, u16, *mut u8, u32) -> u8;
type Vl53Read = extern "C" fn(*mut c_void, u16, *mut u8, u32) -> u8;
type Vl53Wait = extern "C" fn(*mut c_void, u32) -> u8;

#[repr(C)]
struct Vl53l8cxPlatform {
    address: u16,
    write: Vl53Write,
    read: Vl53Read,
    wait: Vl53Wait,
    handle: *mut c_void,
}

#[repr(C)]
struct Vl53l8cxConfiguration {
    platform: Vl53l8cxPlatform,
    streamcount: u8,
    data_read_size: u32,
    default_configuration: *mut u8,
    default_xtalk: *mut u8,
    offset_data: [u8; VL53L8CX_OFFSET_BUFFER_SIZE],
    xtalk_data: [u8; VL53L8CX_XTALK_BUFFER_SIZE],
    temp_buffer: [u8; VL53L8CX_TEMPORARY_BUFFER_SIZE],
    is_auto_stop_enabled: u8,
}

#[repr(C)]
struct Vl53l8cxMotionIndicator {
    global_indicator_1: u32,
    global_indicator_2: u32,
    status: u8,
    nb_of_detected_aggregates: u8,
    nb_of_aggregates: u8,
    spare: u8,
    motion: [u32; 32],
}

#[repr(C)]
struct Vl53l8cxResultsData {
    silicon_temp_degc: i8,
    ambient_per_spad: [u32; VL53L8CX_RESOLUTION_8X8_VALUES],
    nb_target_detected: [u8; VL53L8CX_RESOLUTION_8X8_VALUES],
    nb_spads_enabled: [u32; VL53L8CX_RESOLUTION_8X8_VALUES],
    signal_per_spad: [u32; VL53L8CX_RESOLUTION_8X8_VALUES],
    range_sigma_mm: [u16; VL53L8CX_RESOLUTION_8X8_VALUES],
    distance_mm: [i16; VL53L8CX_RESOLUTION_8X8_VALUES],
    reflectance: [u8; VL53L8CX_RESOLUTION_8X8_VALUES],
    target_status: [u8; VL53L8CX_RESOLUTION_8X8_VALUES],
    motion_indicator: Vl53l8cxMotionIndicator,
}

#[link(name = "vl53l8cx", kind = "static")]
unsafe extern "C" {
    fn vl53l8cx_init(device: *mut Vl53l8cxConfiguration) -> u8;
    fn vl53l8cx_set_resolution(
        device: *mut Vl53l8cxConfiguration,
        resolution: u8,
    ) -> u8;
    fn vl53l8cx_start_ranging(device: *mut Vl53l8cxConfiguration) -> u8;
    fn vl53l8cx_check_data_ready(
        device: *mut Vl53l8cxConfiguration,
        ready: *mut u8,
    ) -> u8;
    fn vl53l8cx_get_ranging_data(
        device: *mut Vl53l8cxConfiguration,
        results: *mut Vl53l8cxResultsData,
    ) -> u8;
}

extern "C" fn vl53_i2c_write(
    handle: *mut c_void,
    register_address: u16,
    values: *mut u8,
    size: u32,
) -> u8 {
    let i2c = unsafe { &mut *(handle as *mut I2c<'static, Blocking>) };
    let values = unsafe { core::slice::from_raw_parts(values, size as usize) };
    let mut offset = 0;

    while offset < values.len() {
        let chunk_len = core::cmp::min(values.len() - offset, 32);
        let mut buffer = [0u8; 258];
        let address = register_address.wrapping_add(offset as u16);
        buffer[0] = (address >> 8) as u8;
        buffer[1] = address as u8;
        buffer[2..2 + chunk_len].copy_from_slice(&values[offset..offset + chunk_len]);

        if let Err(error) = i2c.write(
            VL53L8CX_I2C_ADDRESS,
            &buffer[..2 + chunk_len],
        ) {
            error!(
                "VL53L8CX I2C write failed: register=0x{:04X} size={} error={:?}",
                address,
                chunk_len,
                error,
            );
            return 1;
        }
        offset += chunk_len;
    }

    0
}

extern "C" fn vl53_i2c_read(
    handle: *mut c_void,
    register_address: u16,
    values: *mut u8,
    size: u32,
) -> u8 {
    let i2c = unsafe { &mut *(handle as *mut I2c<'static, Blocking>) };
    let values = unsafe { core::slice::from_raw_parts_mut(values, size as usize) };
    let mut offset = 0;

    while offset < values.len() {
        let chunk_len = core::cmp::min(values.len() - offset, 32);
        let address = register_address.wrapping_add(offset as u16);
        let address_bytes = [(address >> 8) as u8, address as u8];

        if let Err(error) = i2c.write_read(
            VL53L8CX_I2C_ADDRESS,
            &address_bytes,
            &mut values[offset..offset + chunk_len],
        ) {
            error!(
                "VL53L8CX I2C read failed: register=0x{:04X} size={} error={:?}",
                address,
                chunk_len,
                error,
            );
            return 1;
        }
        offset += chunk_len;
    }

    0
}

extern "C" fn vl53_wait(_handle: *mut c_void, milliseconds: u32) -> u8 {
    esp_hal::delay::Delay::new().delay_millis(milliseconds);
    0
}

#[embassy_executor::task]
async fn sensor_task(mut i2c: I2c<'static, Blocking>) {
    let platform = Vl53l8cxPlatform {
        address: 0x52,
        write: vl53_i2c_write,
        read: vl53_i2c_read,
        wait: vl53_wait,
        handle: (&mut i2c as *mut I2c<'static, Blocking>).cast(),
    };

    let mut device_storage = core::mem::MaybeUninit::<Vl53l8cxConfiguration>::uninit();
    let device_ptr = device_storage.as_mut_ptr();
    unsafe {
        core::ptr::addr_of_mut!((*device_ptr).platform).write(platform);
    }

    let init_status = unsafe { vl53l8cx_init(device_ptr) };
    if init_status != 0 {
        error!("VL53L8CX init failed: {}", init_status);
        SYSTEM_STATE.store(SystemState::SensorError as u8, Ordering::Relaxed);
        return;
    }

    let mut device = unsafe { device_storage.assume_init() };

    let resolution_status = unsafe {
        vl53l8cx_set_resolution(&mut device, VL53L8CX_RESOLUTION_8X8)
    };
    if resolution_status != 0 {
        error!("VL53L8CX set 8x8 resolution failed: {}", resolution_status);
        SYSTEM_STATE.store(SystemState::SensorError as u8, Ordering::Relaxed);
        return;
    }

    let start_status = unsafe { vl53l8cx_start_ranging(&mut device) };
    if start_status != 0 {
        error!("VL53L8CX start failed: {}", start_status);
        SYSTEM_STATE.store(SystemState::SensorError as u8, Ordering::Relaxed);
        return;
    }

    info!("VL53L8CX ranging started");
    let mut results: Vl53l8cxResultsData = unsafe { core::mem::zeroed() };

    loop {
        let mut ready = 0;
        let ready_status = unsafe { vl53l8cx_check_data_ready(&mut device, &mut ready) };

        if ready_status != 0 {
            error!("VL53L8CX ready check failed: {}", ready_status);
        } else if ready != 0 {
            let data_status = unsafe {
                vl53l8cx_get_ranging_data(&mut device, &mut results)
            };
            if data_status == 0 {
                info!(
                    "VL53L8CX distance: {} mm, status: {}, temperature: {}, ({})",
                    results.distance_mm[0],
                    results.target_status[0],
                    results.silicon_temp_degc,
                    results.distance_mm.len()
                );
                info!(
                    "{} {} {} {} {} {} {} {}",
                    left_pad(results.distance_mm[0]),
                    left_pad(results.distance_mm[1]),
                    left_pad(results.distance_mm[2]),
                    left_pad(results.distance_mm[3]),
                    left_pad(results.distance_mm[4]),
                    left_pad(results.distance_mm[5]),
                    left_pad(results.distance_mm[6]),
                    left_pad(results.distance_mm[7])
                );
                info!(
                    "{} {} {} {} {} {} {} {}",
                    left_pad(results.distance_mm[8]),
                    left_pad(results.distance_mm[9]),
                    left_pad(results.distance_mm[10]),
                    left_pad(results.distance_mm[11]),
                    left_pad(results.distance_mm[12]),
                    left_pad(results.distance_mm[13]),
                    left_pad(results.distance_mm[14]),
                    left_pad(results.distance_mm[15])
                );
                info!(
                    "{} {} {} {} {} {} {} {}",
                    left_pad(results.distance_mm[16]),
                    left_pad(results.distance_mm[17]),
                    left_pad(results.distance_mm[18]),
                    left_pad(results.distance_mm[19]),
                    left_pad(results.distance_mm[20]),
                    left_pad(results.distance_mm[21]),
                    left_pad(results.distance_mm[22]),
                    left_pad(results.distance_mm[23])
                );
                info!(
                    "{} {} {} {} {} {} {} {}",
                    left_pad(results.distance_mm[24]),
                    left_pad(results.distance_mm[25]),
                    left_pad(results.distance_mm[26]),
                    left_pad(results.distance_mm[27]),
                    left_pad(results.distance_mm[28]),
                    left_pad(results.distance_mm[29]),
                    left_pad(results.distance_mm[30]),
                    left_pad(results.distance_mm[31])
                );
                info!(
                    "{} {} {} {} {} {} {} {}",
                    left_pad(results.distance_mm[32]),
                    left_pad(results.distance_mm[33]),
                    left_pad(results.distance_mm[34]),
                    left_pad(results.distance_mm[35]),
                    left_pad(results.distance_mm[36]),
                    left_pad(results.distance_mm[37]),
                    left_pad(results.distance_mm[38]),
                    left_pad(results.distance_mm[39])
                );
                info!(
                    "{} {} {} {} {} {} {} {}",
                    left_pad(results.distance_mm[40]),
                    left_pad(results.distance_mm[41]),
                    left_pad(results.distance_mm[42]),
                    left_pad(results.distance_mm[43]),
                    left_pad(results.distance_mm[44]),
                    left_pad(results.distance_mm[45]),
                    left_pad(results.distance_mm[46]),
                    left_pad(results.distance_mm[47])
                );
                info!(
                    "{} {} {} {} {} {} {} {}",
                    left_pad(results.distance_mm[48]),
                    left_pad(results.distance_mm[49]),
                    left_pad(results.distance_mm[50]),
                    left_pad(results.distance_mm[51]),
                    left_pad(results.distance_mm[52]),
                    left_pad(results.distance_mm[53]),
                    left_pad(results.distance_mm[54]),
                    left_pad(results.distance_mm[55])
                );
                info!(
                    "{} {} {} {} {} {} {} {}",
                    left_pad(results.distance_mm[56]),
                    left_pad(results.distance_mm[57]),
                    left_pad(results.distance_mm[58]),
                    left_pad(results.distance_mm[59]),
                    left_pad(results.distance_mm[60]),
                    left_pad(results.distance_mm[61]),
                    left_pad(results.distance_mm[62]),
                    left_pad(results.distance_mm[63])
                );
            } else {
                error!("VL53L8CX data read failed: {}", data_status);
            }
        }

        Timer::after(Duration::from_millis(20)).await;
    }
}

fn left_pad(number: i16) -> heapless::String<24> {
    let color = match number {
        ..=49 => "\x1b[38;5;196m",       // red
        50..=99 => "\x1b[38;5;208m",     // orange
        100..=199 => "\x1b[38;5;226m",   // yellow
        200..=399 => "\x1b[38;5;46m",    // green
        400..=799 => "\x1b[38;5;51m",    // cyan
        800..=1599 => "\x1b[38;5;21m",   // blue
        1600..=3900 => "\x1b[38;5;129m", // violet-blue
        _ => "\x1b[38;5;129m",            // violet
    };

    let mut padded = heapless::String::<24>::new();
    write!(padded, "{}{:04}\x1b[0m", color, number).unwrap();
    padded
}