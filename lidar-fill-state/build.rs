/// The path of the file in which the wifi-config are
/// 
/// In the file should be the following vlaues like `KEY=Value`:
/// 
/// |Name|Description|
/// |-|-|
/// |WIFI_SSID|The name of the wlan to connect to|
/// |WIFI_PASSWORD|The password to use|
const WIFI_FILE_PATH: &str = "wifi.env";

/// The path of the csv-file in which the partition-tables are
const PARTITION_TABLE_FILE_PATH: &str = "partition-table.csv";

fn main() {
    linker_be_nice();

    cc::Build::new()
        .compiler("riscv32-esp-elf-gcc")
        .files([
            "csrc/vl53l8cx_api.c",
            "csrc/vl53l8cx_platform.c",
        ])
        .include("csrc")
        .define("VL53L8CX_NB_TARGET_PER_ZONE", "1")
        .flag("-ffreestanding")
        .compile("vl53l8cx");

    println!("cargo:rerun-if-changed=csrc");
    println!("cargo:rustc-link-arg=-Tlinkall.x");

    load_wifi_credentials();
    load_storage_indexes_from_partition_table();
}

/// Loads the wifi-config from the file.
/// 
/// The config values will be added to the rustc-env (`WIFI_SSID` and `WIFI_PASSWORD`)
fn load_wifi_credentials() {
    println!("cargo:rerun-if-changed={WIFI_FILE_PATH}");

    let contents = std::fs::read_to_string(WIFI_FILE_PATH)
        .expect(format!("Failed to read {}", WIFI_FILE_PATH).as_str());

    for line in contents.lines() {
        let line = line.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            panic!("Invalid line in {WIFI_FILE_PATH}: {line}");
        };

        let key = key.trim();
        let value = value.trim();

        match key {
            "WIFI_SSID" | "WIFI_PASSWORD" => {
                println!("cargo:rustc-env={key}={value}");
            }
            _ => {
                panic!("Unknown key in {WIFI_FILE_PATH}: {key}");
            }
        }
    }
}

/// Loads the start and end index of the storage partition from the file.
/// 
/// The indexes will be added to the rustc-env (`STORAGE_PARTITION_START_INDEX` and `STORAGE_PARTITION_END_INDEX`)
fn load_storage_indexes_from_partition_table() {
    println!("cargo:rerun-if-changed={PARTITION_TABLE_FILE_PATH}");

    let content = std::fs::read_to_string(PARTITION_TABLE_FILE_PATH)
        .expect(format!("Failed to read {}", PARTITION_TABLE_FILE_PATH).as_str());

    const STORAGE_PARTION_NAME: &str = "storage";
    let mut found_storage_size = false;

    for line in content.lines() {
        let line = line.trim();

        if line.starts_with(STORAGE_PARTION_NAME) == false {
            continue;
        }

        found_storage_size = true;
        let line_split: Vec<_> = line
            .split(",")
            .skip_while(|x| x.is_empty())
            .map(str::trim)
            .collect();

        if line_split.len() < 5 {
            panic!("The definition of the partition \"{}\" has an unexpected format", STORAGE_PARTION_NAME);
        }

        let start_index = line_split[3];
        let end_index = line_split[4];

        println!("cargo:rustc-env=STORAGE_PARTITION_START_INDEX={start_index}");
        println!("cargo:rustc-env=STORAGE_PARTITION_END_INDEX={end_index}");
    }

    if found_storage_size == false {
        panic!("No partition with the name \"{STORAGE_PARTION_NAME}\" found");
    }
}

fn linker_be_nice() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let kind = &args[1];
        let what = &args[2];

        match kind.as_str() {
            "undefined-symbol" => match what.as_str() {
                what if what.starts_with("_defmt_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `defmt` not found - make sure `defmt.x` is added as a linker script and you have included `use defmt_rtt as _;`"
                    );
                    eprintln!();
                }
                "_stack_start" => {
                    eprintln!();
                    eprintln!("💡 Is the linker script `linkall.x` missing?");
                    eprintln!();
                }
                what if what.starts_with("esp_rtos_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `esp-radio` has no scheduler enabled. Make sure you have initialized `esp-rtos` or provided an external scheduler."
                    );
                    eprintln!();
                }
                "embedded_test_linker_file_not_added_to_rustflags" => {
                    eprintln!();
                    eprintln!(
                        "💡 `embedded-test` not found - make sure `embedded-test.x` is added as a linker script for tests"
                    );
                    eprintln!();
                }
                "free"
                | "malloc"
                | "calloc"
                | "get_free_internal_heap_size"
                | "malloc_internal"
                | "realloc_internal"
                | "calloc_internal"
                | "free_internal" => {
                    eprintln!();
                    eprintln!(
                        "💡 Did you forget the `esp-alloc` dependency or didn't enable the `compat` feature on it?"
                    );
                    eprintln!();
                }
                _ => (),
            },
            // we don't have anything helpful for "missing-lib" yet
            _ => {
                std::process::exit(1);
            }
        }

        std::process::exit(0);
    }

    println!(
        "cargo:rustc-link-arg=--error-handling-script={}",
        std::env::current_exe().unwrap().display()
    );
}
