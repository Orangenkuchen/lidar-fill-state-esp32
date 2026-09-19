// Don't use Rust's standard library because ESP32 dont support it
#![no_std]
// Don't use Rust's standard main entrypoint (entry will be defined with [esp_rtos::main]).
#![no_main]

// Embassy is an asynchronous embedded framework 
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};

// ESP_HAL interacts with the hardware components of the esp32
use esp_hal::{
    clock::CpuClock,
    gpio::Level,
    rmt::{Channel, PulseCode, Rmt, Tx, TxChannelConfig, TxChannelCreator},
    time::Rate,
    timer::timg::TimerGroup,
};

use heapless::Vec;

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

    // This creates a heap. The heap is used by things that require dynamic allocation.
    esp_alloc::heap_allocator!(
        #[esp_hal::ram(reclaimed)]
        size: 65536
    );

    // Start Embassy runtime with the ESP32's Timergroup 0.
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    // Start the Runtime and CPU interrupts
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    // Initializes the ESP32-C6's RMT hardware. Use RMT at 80 MHz.
    let rmt = Rmt::new(
        peripherals.RMT,
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
        .with_pin(peripherals.GPIO8);


    // --------------------------------------------------------
    // Start LED task
    // --------------------------------------------------------

    spawner.spawn(
        led_task(channel)
            .expect("failed to create LED task")
    );



    // --------------------------------------------------------
    // Main task
    // --------------------------------------------------------
    //
    // Later we can spawn things like:
    //
    // spawner.spawn(wifi_task(...)).unwrap();
    // spawner.spawn(web_server_task(...)).unwrap();
    // spawner.spawn(lidar_task(...)).unwrap();
    //
    // The main task itself doesn't need to do anything right
    // now, so it simply stays alive.
    //

    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
}


/// ============================================================
/// LED TASK
/// ============================================================
///
/// This runs independently from main().
///
/// Because the RMT channel is asynchronous, while the RMT
/// hardware is transmitting, Embassy can run other tasks.
#[embassy_executor::task]
async fn led_task(
    mut channel: Channel<'static, esp_hal::Async, Tx>,
) {
    loop {
        show_led_sequence(
            &mut channel, 
            [
                LedFlash { r: 0x05, g: 0x0, b: 0x05, duration: Duration::from_millis(500) },
                LedFlash { r: 0x05, g: 0x05, b: 0x0, duration: Duration::from_millis(500) }
            ].into()
        ).await;
    }
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