// Don't use Rust's standard library because ESP32 doesn't support it.
#![no_std]
// Don't use Rust's standard main entrypoint.
// The entrypoint is provided by #[esp_rtos::main].
#![no_main]

use core::fmt::{Write as FmtWrite};
use core::mem::size_of;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use edge_http::{
    io::{
        server::{
            DefaultServer
        }
    }
};
use edge_nal::TcpBind;
use edge_nal_embassy::{
    Tcp,
    TcpBuffers,
};
use esp_radio::wifi::DisconnectReason;
use lidar_fill_state::modules::http_handler::HttpHandler;
use lidar_fill_state::modules::little_fs_storage::LittleFsStorage;
use lidar_fill_state::modules::vl53l8cx_lidar_service::{
    CheckForNewDataError,
    InitError,
    StartRangingError,
    StopRangingError,
    VL53L8CxLidarService,
};
use log::{debug, error, info, warn, trace};
use embassy_executor::Spawner;
use embassy_net::{
    Config as NetConfig,
    Runner,
    Stack,
    StackResources,
};
use embassy_time::{Duration, Timer};
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
    Config, Filesystem
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

/// The size of a storage block
const STORAGE_BLOCK_SIZE: u32 = 4 * 1_024;
/// The amount of blocks in the storage
const STORAGE_BLOCK_COUNT: u32 = 528;
/// The start index of the storage partition
const STORAGE_PARTITION_START_INDEX: &str = env!("STORAGE_PARTITION_START_INDEX");
/// The size of the cache for the file system
const FILE_SYSTEM_CACHE_SIZE: u32 = 4 * 1_024;
/// The ssid of the wlan to connect to
const WIFI_SSID: &str = env!("WIFI_SSID");
/// The wifi password to use
const WIFI_PASSWORD: &str = env!("WIFI_PASSWORD");

/// If true the lidar will start the ranging measurment. If it switches to false the measument will stop
static LIDAR_RUN_STATE: AtomicBool = AtomicBool::new(false);
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
    let channel = rmt
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

    let lidar_service = VL53L8CxLidarService::new(i2c);

    debug!("Spawning VL53L8CX sensor task...");
    spawner.spawn(
        sensor_task(
            lidar_service
        ).expect("failed to spawn sensor task")
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

fn init_filesystem(
    flash: esp_hal::peripherals::FLASH<'static>,
) -> Filesystem<LittleFsStorage<'static>> {
    let storage_partition_start_index = u32::from_str_radix(
        STORAGE_PARTITION_START_INDEX.trim_start_matches("0x"),
        16,
    )
    .unwrap();

    let storage = LittleFsStorage::new(
        FlashStorage::new(flash),
        STORAGE_BLOCK_SIZE,
        storage_partition_start_index
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

#[embassy_executor::task]
async fn sensor_task(mut lidar_service: VL53L8CxLidarService) {
    LIDAR_RUN_STATE.store(true, Ordering::Relaxed);

    match lidar_service.init() {
        Ok(()) => {},
        Err(err) => {
            match err {
                InitError::DeviceInitFailed { status } => {
                    error!("The initilization of the lidar failed with status {}", status);
                    return;
                }
                InitError::SettingResolutionFailed { status } => {
                    error!("Error while trying to get the lidar device config (Status: {})", status);
                    return;
                }
            }
        }
    };

    loop {
        let should_run = LIDAR_RUN_STATE.load(Ordering::Relaxed);

        if lidar_service.is_running != should_run {
            match should_run {
                true => {
                    match lidar_service.start_ranging() {
                        Ok(()) => info!("VL53L8CX ranging started"),
                        Err(error) => {
                            match error {
                                StartRangingError::NotInitilized => {
                                    error!("Couldn't start rainging. The lidar is not initialize.");
                                    return;
                                }
                                StartRangingError::StartRangingFailed { status } => {
                                    error!("Error while starting the ranging in the lidar (Status: {})", status);
                                    return;
                                }
                            }
                        }
                    };
                }
                false => {
                    match lidar_service.stop_ranging() {
                        Ok(()) => info!("VL53L8CX ranging stopped"),
                        Err(error) => {
                            match error {
                                StopRangingError::NotInitilized => {
                                    error!("Couldn't stop rainging. The lidar is not initialize.");
                                    return;
                                }
                                StopRangingError::StopRangingFailed { status } => {
                                    error!("Error while stopping the ranging in the lidar (Status: {})", status);
                                    return;
                                }
                            }
                        }
                    };
                }
            }
        }

        if lidar_service.is_running {
            let mut do_read_values = false;
            match lidar_service.check_for_new_data() {
                Ok(has_new_data) => do_read_values = has_new_data,
                Err(error) => {
                    match error {
                        CheckForNewDataError::NotInitilized => {
                            error!("Couldn't check for new lidar data. The lidar is not initialize.");
                        },
                        CheckForNewDataError::ReadyError { status } => {
                            error!("While checking if the lidar is ready it returned Status {}", status);
                        },
                        CheckForNewDataError::DataError { status } => {
                            error!("While reading lidar data it returned Status {}", status);
                        }
                    }
                }
            }

            if do_read_values {
                let read_value = match lidar_service.getting_lidar_reading() {
                    Some(mut receiver) => match receiver.try_get() {
                        None => continue,
                        Some(value) => value,
                    },
                    None => {
                        error!("The lidar receiver couln't be received. Is the LidarService initilized?");
                        return;
                    }
                };

                info!(
                        "VL53L8CX distance: {} mm, status: {}, temperature: {}",
                        read_value.zones[0].distance_mm,
                        read_value.zones[0].target_status,
                        read_value.silicon_temp_degc
                );
                info!(
                    "{} {} {} {} {} {} {} {}",
                    left_pad(read_value.zones[0].distance_mm),
                    left_pad(read_value.zones[1].distance_mm),
                    left_pad(read_value.zones[2].distance_mm),
                    left_pad(read_value.zones[3].distance_mm),
                    left_pad(read_value.zones[4].distance_mm),
                    left_pad(read_value.zones[5].distance_mm),
                    left_pad(read_value.zones[6].distance_mm),
                    left_pad(read_value.zones[7].distance_mm)
                );
                info!(
                    "{} {} {} {} {} {} {} {}",
                    left_pad(read_value.zones[8].distance_mm),
                    left_pad(read_value.zones[9].distance_mm),
                    left_pad(read_value.zones[10].distance_mm),
                    left_pad(read_value.zones[11].distance_mm),
                    left_pad(read_value.zones[12].distance_mm),
                    left_pad(read_value.zones[13].distance_mm),
                    left_pad(read_value.zones[14].distance_mm),
                    left_pad(read_value.zones[15].distance_mm)
                );
                info!(
                    "{} {} {} {} {} {} {} {}",
                    left_pad(read_value.zones[16].distance_mm),
                    left_pad(read_value.zones[17].distance_mm),
                    left_pad(read_value.zones[18].distance_mm),
                    left_pad(read_value.zones[19].distance_mm),
                    left_pad(read_value.zones[20].distance_mm),
                    left_pad(read_value.zones[21].distance_mm),
                    left_pad(read_value.zones[22].distance_mm),
                    left_pad(read_value.zones[23].distance_mm)
                );
                info!(
                    "{} {} {} {} {} {} {} {}",
                    left_pad(read_value.zones[24].distance_mm),
                    left_pad(read_value.zones[25].distance_mm),
                    left_pad(read_value.zones[26].distance_mm),
                    left_pad(read_value.zones[27].distance_mm),
                    left_pad(read_value.zones[28].distance_mm),
                    left_pad(read_value.zones[29].distance_mm),
                    left_pad(read_value.zones[30].distance_mm),
                    left_pad(read_value.zones[31].distance_mm)
                );
                info!(
                    "{} {} {} {} {} {} {} {}",
                    left_pad(read_value.zones[32].distance_mm),
                    left_pad(read_value.zones[33].distance_mm),
                    left_pad(read_value.zones[34].distance_mm),
                    left_pad(read_value.zones[35].distance_mm),
                    left_pad(read_value.zones[36].distance_mm),
                    left_pad(read_value.zones[37].distance_mm),
                    left_pad(read_value.zones[38].distance_mm),
                    left_pad(read_value.zones[39].distance_mm)
                );
                info!(
                    "{} {} {} {} {} {} {} {}",
                    left_pad(read_value.zones[40].distance_mm),
                    left_pad(read_value.zones[41].distance_mm),
                    left_pad(read_value.zones[42].distance_mm),
                    left_pad(read_value.zones[43].distance_mm),
                    left_pad(read_value.zones[44].distance_mm),
                    left_pad(read_value.zones[45].distance_mm),
                    left_pad(read_value.zones[46].distance_mm),
                    left_pad(read_value.zones[47].distance_mm)
                );
                info!(
                    "{} {} {} {} {} {} {} {}",
                    left_pad(read_value.zones[48].distance_mm),
                    left_pad(read_value.zones[49].distance_mm),
                    left_pad(read_value.zones[50].distance_mm),
                    left_pad(read_value.zones[51].distance_mm),
                    left_pad(read_value.zones[52].distance_mm),
                    left_pad(read_value.zones[53].distance_mm),
                    left_pad(read_value.zones[54].distance_mm),
                    left_pad(read_value.zones[55].distance_mm)
                );
                info!(
                    "{} {} {} {} {} {} {} {}",
                    left_pad(read_value.zones[56].distance_mm),
                    left_pad(read_value.zones[57].distance_mm),
                    left_pad(read_value.zones[58].distance_mm),
                    left_pad(read_value.zones[59].distance_mm),
                    left_pad(read_value.zones[60].distance_mm),
                    left_pad(read_value.zones[61].distance_mm),
                    left_pad(read_value.zones[62].distance_mm),
                    left_pad(read_value.zones[63].distance_mm)
                );
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