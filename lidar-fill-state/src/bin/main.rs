// Don't use Rust's standard library because ESP32 dont support it
#![no_std]
// Don't use Rust's standard main entrypoint (entry will be defined with [esp_rtos::main]).
#![no_main]

use core::sync::atomic::{AtomicU8, Ordering};

// Embassy is an asynchronous embedded framework 
use embassy_executor::Spawner;
use embassy_net::{
    tcp::TcpSocket,
    Config as NetConfig,
    Runner,
    Stack,
    StackResources,
};
use embassy_time::{Duration, Timer};
use embedded_io_async::{Read, Write};

// ESP_HAL interacts with the hardware components of the esp32
use esp_hal::{
    clock::CpuClock, gpio::Level, peripherals::{Peripherals, WIFI}, rmt::{
        Channel,
        PulseCode,
        Rmt,
        Tx,
        TxChannelConfig,
        TxChannelCreator,
    }, rng::Rng, time::Rate, timer::timg::TimerGroup,
};

use heapless::Vec;

use esp_radio::wifi::{
    AuthenticationMethodConfig,
    Config as WifiConfig,
    Interface,
    WifiController,
    sta::StationConfig,
};

use static_cell::StaticCell;

const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASSWORD: &str = env!("WIFI_PASSWORD");

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum SystemState {
    Starting = 0,
    Running = 1,
    WifiError = 2,
    SensorError = 3,
}

static SYSTEM_STATE: AtomicU8 = AtomicU8::new(SystemState::Starting as u8);


/// ============================================================
/// STATIC NETWORK MEMORY
/// ============================================================
///
/// embassy-net needs memory that lives for the entire lifetime
/// of the program.
///
/// StackResources<4> gives the network stack room for several
/// sockets.
///
static STACK_RESOURCES: StaticCell<StackResources<4>> =
    StaticCell::new();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // TODO: Handle error better. Maybe LED?
    loop {}
}

// Put the application metadata into the firmware in the format the ESP32 bootloader expects.
esp_bootloader_esp_idf::esp_app_desc!();

// This macro sets up the ESP RTOS/Embassy runtime.
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // Use the default configuration, but run the CPU at its maximum clock speed.
    let config = esp_hal::Config::default()
        .with_cpu_clock(CpuClock::max());

    // initializes the ESP32 hardware and gives us access to its peripherals.
    let peripherals = esp_hal::init(config);
    let esp_hal::peripherals::Peripherals {
        GPIO8,
        WIFI,
        RMT,
        TIMG0,
        FROM_CPU_INTR0,
        ..
    } = peripherals;

    // This creates a heap. The heap is used by things that require dynamic allocation.
    esp_alloc::heap_allocator!(
        #[esp_hal::ram(reclaimed)]
        size: 65536
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


    // Start the LED-Task
    spawner.spawn(
        led_task(channel)
            .expect("failed to create LED task")
    );

    let network_components = setup_network(WIFI);

    // START NETWORK TASK
    //
    // The runner MUST run continuously.
    //
    // Without this task, TCP/IP won't actually process packets.
    spawner.spawn(
        net_task(network_components.runner)
            .expect("failed to spawn network task")
    );

    // Start the wifi task
    spawner.spawn(
        wifi_task(network_components.wifi_controller)
            .expect("failed to spawn Wi-Fi task")
    );

    // WAIT FOR DHCP
    //
    // This suspends main until the ESP32 has obtained an
    // IPv4 configuration from the router.
    network_components.stack.wait_config_up().await;

    // START WEB SERVER
    //
    // At this point:
    //
    //     Wi-Fi connected
    //          +
    //     DHCP completed
    //          +
    //     TCP/IP stack running
    //
    // So we can start accepting HTTP connections.
    spawner.spawn(
        web_server_task(network_components.stack)
            .expect("failed to spawn web server task")
    );

    // MAIN TASK
    //
    // Everything important is now running in Embassy tasks:
    //
    //     LED
    //     Wi-Fi
    //     TCP/IP
    //     HTTP server
    //
    // Keep main alive.
    //

    loop {
        Timer::after(
            Duration::from_secs(60)
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
        
        let ledSequence: Vec<LedFlash, 10> = match state {
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
            ledSequence
        ).await;
    }
}

/// This Task continously tries to connect connect to the Wifi.
/// On a disconnect it tries to reconnect.
#[embassy_executor::task]
async fn wifi_task(
    mut controller: WifiController<'static>) -> ! {

    loop {

        // Try to connect.
        match controller.connect_async().await {

            Ok(_) => {
                // Successfully connected.
                //
                // The network stack can now communicate with
                // the router.
                SYSTEM_STATE.store(
                    SystemState::Running as u8,
                    Ordering::Relaxed,
                );
            }

            Err(_) => {
                // Connection failed.
                //
                // Wait before trying again.
                SYSTEM_STATE.store(
                    SystemState::WifiError as u8,
                    Ordering::Relaxed,
                );

                Timer::after(Duration::from_secs(5)).await;

                continue;
            }
        }

        // Wait until the connection disappears.
        let _ =
            controller
                .wait_for_disconnect_async()
                .await;

        // Connection lost.
        //
        // Loop around and reconnect.
        Timer::after(Duration::from_secs(2)).await;
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
    stack: Stack<'static>) -> ! {

    // TCP receive/transmit buffers.
    //
    // These are reused for every connection.
    let mut rx_buffer =
        [0u8; 2048];

    let mut tx_buffer =
        [0u8; 2048];

    loop {

        // Create TCP socket
        let mut socket =
            TcpSocket::new(
                stack,
                &mut rx_buffer,
                &mut tx_buffer,
            );

        // Don't allow a client to keep the socket open
        // forever without sending anything.
        socket.set_timeout(
            Some(
                Duration::from_secs(10)
            )
        );

        // Listen on HTTP port 80
        if socket
            .accept(80)
            .await
            .is_err()
        {
            continue;
        }

        // Receive HTTP request
        let mut request =
            [0u8; 1024];

        let mut request_len = 0usize;


        loop {

            if request_len >= request.len() {
                break;
            }

            match socket
                .read(
                    &mut request[request_len..]
                )
                .await
            {

                Ok(0) => {
                    break;
                }

                Ok(n) => {

                    request_len += n;

                    // HTTP headers end with:
                    //
                    //     \r\n\r\n
                    if request[..request_len]
                        .windows(4)
                        .any(|x| x == b"\r\n\r\n")
                    {
                        break;
                    }
                }

                Err(_) => {
                    break;
                }
            }
        }

        // Parse HTTP request
        let request =
            core::str::from_utf8(
                &request[..request_len]
            )
            .unwrap_or("");

        // HTTP request looks like:
        //
        //     GET / HTTP/1.1
        //
        // The second whitespace-separated item is the path.
        let path =
            request
                .split_whitespace()
                .nth(1)
                .unwrap_or("/");

        // Generate response
        let (content_type, body) =
            match path {

                "/" => (
                    "text/html",
                    "\
<!DOCTYPE html>
<html>
<head>
    <meta charset=\"utf-8\">
    <title>ESP32-C6</title>
</head>

<body>

    <h1>ESP32-C6</h1>

    <p>Web server is running.</p>

    <p>
        <a href=\"/hello\">
            Test endpoint
        </a>
    </p>

</body>
</html>
",
                ),

                "/hello" => (
                    "text/plain",
                    "Hello from the ESP32-C6!\n",
                ),

                _ => (
                    "text/plain",
                    "404 Not Found\n",
                ),
            };

        // HTTP response header
        let status =
            if path == "/"
                || path == "/hello"
            {
                "200 OK"
            } else {
                "404 Not Found"
            };

        // Build response header.
        //
        // We don't need dynamic allocation here.
        let mut header =
            [0u8; 256];

        let header_text =
            format_http_header(
                &mut header,
                status,
                content_type,
                body.len(),
            );

        // Send response
        let _ =
            socket
                .write_all(header_text)
                .await;

        let _ =
            socket
                .write_all(body.as_bytes())
                .await;

        let _ =
            socket.flush().await;


        //
        // Connection is closed automatically when the socket
        // goes out of scope.
        //
    }
}

/// HTTP HEADER BUILDER
fn format_http_header<'a>(
    buffer: &'a mut [u8],
    status: &str,
    content_type: &str,
    content_length: usize) -> &'a [u8] {

    // We construct:
    //
    // HTTP/1.1 200 OK
    // Content-Type: text/html
    // Content-Length: 123
    // Connection: close
    //
    // ...
    let mut pos = 0;


    pos += copy_into(
        &mut buffer[pos..],
        b"HTTP/1.1 ",
    );

    pos += copy_into(
        &mut buffer[pos..],
        status.as_bytes(),
    );

    pos += copy_into(
        &mut buffer[pos..],
        b"\r\nContent-Type: ",
    );

    pos += copy_into(
        &mut buffer[pos..],
        content_type.as_bytes(),
    );

    pos += copy_into(
        &mut buffer[pos..],
        b"\r\nContent-Length: ",
    );


    pos += write_number(
        &mut buffer[pos..],
        content_length,
    );


    pos += copy_into(
        &mut buffer[pos..],
        b"\r\nConnection: close\r\n\r\n",
    );


    &buffer[..pos]
}

/// COPY BYTES
fn copy_into(
    destination: &mut [u8],
    source: &[u8]) -> usize {

    let length =
        core::cmp::min(
            destination.len(),
            source.len(),
        );

    destination[..length]
        .copy_from_slice(
            &source[..length]
        );

    length
}

/// WRITE DECIMAL NUMBER
fn write_number(
    buffer: &mut [u8],
    mut number: usize) -> usize {

    if number == 0 {
        buffer[0] = b'0';
        return 1;
    }

    let mut digits =
        [0u8; 20];

    let mut count = 0;

    while number > 0 {

        digits[count] =
            b'0' + (number % 10) as u8;

        number /= 10;

        count += 1;
    }

    // Reverse digits.
    let mut i = 0;

    while i < count {

        buffer[i] =
            digits[count - 1 - i];

        i += 1;
    }

    count
}

/// ============================================================
/// WS2812 / ADDRESSABLE RGB LED
/// ============================================================
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