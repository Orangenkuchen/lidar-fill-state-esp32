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
    rmt::{PulseCode, Rmt, TxChannelConfig, TxChannelCreator}, // The RGB-LED speeks the RMT-Protocoll
    time::Rate,
    timer::timg::TimerGroup,
};


#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // TODO: Handle error better. Maybe LED?
    loop {}
}

// Put the application metadata into the firmware in the format the ESP32 bootloader expects.
esp_bootloader_esp_idf::esp_app_desc!();

// This macro sets up the ESP RTOS/Embassy runtime.
#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
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

    loop {
        // RED
        let data = ws2812_data(5, 0, 5);

        channel.transmit(&data).await.unwrap();

        Timer::after(Duration::from_millis(500)).await;

        // YELLOW
        let data = ws2812_data(5, 5, 0);

        channel.transmit(&data).await.unwrap();

        Timer::after(Duration::from_millis(500)).await;
    }
}

/// Generate the RMT waveform for one WS2812 RGB LED.
///
/// WS2812 expects colors in GRB order.
fn ws2812_data(r: u8, g: u8, b: u8) -> [PulseCode; 25] {
    // RGB = 3 bytes so 24 bit
    // 24 data bits + 1 reset = 25 PulseCodes


    let bytes = [g, r, b];

    let mut data = [PulseCode::default(); 25];

    let mut bit = 0;

    while bit < 24 {
        // Select the current byte (becuase divide discards floatvalues on purpose here)
        let byte = bytes[bit / 8];
        // The Mask starts at the most left hand 
        // bit and moves right with each iterration
        let mask = 0x80 >> (bit % 8);

        if byte & mask != 0 {
            // Bit 1:
            // ~0.8 us HIGH + ~0.45 us LOW
            data[bit] = PulseCode::new(
                Level::High,
                64,
                Level::Low,
                36,
            );
        } else {
            // Bit 0:
            // ~0.4 us HIGH + ~0.85 us LOW
            data[bit] = PulseCode::new(
                Level::High,
                32,
                Level::Low,
                68,
            );
        }

        bit += 1;
    }

    // Reset/latch: LOW for >50 us.
    data[24] = PulseCode::new(
        Level::Low,
        4000,
        Level::Low,
        0,
    );

    data
}
