use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::{Receiver, Watch}};
use esp_hal::{Blocking, i2c::master::I2c};
use core::ffi::c_void;
use embassy_time::Instant;

/// The errors that can happen while initilizing the lidar
#[derive(Debug)]
pub enum InitError {
    /// The initialisation of the lidar failed
    DeviceInitFailed { status: u8 },
    /// Setting the resolution of the lidar failed
    SettingResolutionFailed { status: u8 },
}

/// The errors that can happen while starting the ranging the lidar
#[derive(Debug)]
pub enum StartRangingError {
    /// The lidar is not initilized
    NotInitilized,
    /// The starting of the range readindg of the lidar failed
    StartRangingFailed { status: u8 },
}

/// The errors that can happen while stopping the ranging the lidar
#[derive(Debug)]
pub enum StopRangingError {
    /// The lidar is not initilized
    NotInitilized,
    /// The stopping of the range readindg of the lidar failed
    StopRangingFailed { status: u8 },
}
/// The errors that can happen while checking for new data the lidar
#[derive(Debug)]
pub enum CheckForNewDataError {
    /// The lidar is not initilized
    NotInitilized,
    /// Checking if the lidar is ready failed
    ReadyError { status: u8 },
    /// Reading the data form the lidar failed
    DataError { status: u8 },
}

/// The data of a zone from the lidar
#[derive(Clone, Copy)]
pub struct LidarZone {
    /// Ambient noise in kcps/spad
    pub ambient_per_spad: u32,
    /// Number of valid targets detected
    pub nb_target_detected: u8,
    /// Number of SPADs enabled
    pub nb_spads_enabled: u32,
    /// Signal returned to the sensor, in kcps/spad
    pub signal_per_spad: u32,
    /// Sigma of the current distance in mm; lower is better / more stable
    pub range_sigma_mm: u16,
    /// Measured distance in mm
    pub distance_mm: i16,
    /// Estimated reflectance in percent
    pub reflectance: u8,
    /// Status; values 5 and 9 indicate valid ranging
    pub target_status: u8,
}

/// A reading form the lidar sensor
#[derive(Clone, Copy)]
pub struct LidarReading {
    /// The measurment zones of the lidar
    pub zones: [LidarZone; VL53L8CX_RESOLUTION_8X8_VALUES],

    /// The temperature of the silicon chip
    pub silicon_temp_degc: i8,

    /// The timestamp when `last_distance_values` was last set
    pub last_read_timestamp: Instant
}

/// Service for the lidar
pub struct VL53L8CxLidarService {
    /// The I²C connection to the lidar
    i2c: I2c<'static, Blocking>,

    device_configuration: Option<Vl53l8cxConfiguration>,

    /// The device configuraiton of the lidar
    pub lidar_reading: Watch<CriticalSectionRawMutex, LidarReading, 4>,

    /// Show that the ranging is running
    pub is_running: bool
}

impl VL53L8CxLidarService {
    /// Initializes the Struct
    /// 
    /// ### Parameters:
    /// **spawner**: Spawner that will be used inside the service
    /// **i2c**: The I²C connection to the lidar
    pub fn new(i2c: I2c<'static, Blocking>) -> Self {
        Self {
            i2c,
            device_configuration: None,
            lidar_reading: Watch::new(),
            is_running: false
        }
    }

    /// Initializes the Sensor
    /// 
    /// Returns errors when init fails or the setting of the resolution.
    pub fn init(&mut self) -> Result<(), InitError> {
        let platform = Vl53l8cxPlatform {
            address: 0x52,
            write: vl53_i2c_write,
            read: vl53_i2c_read,
            wait: vl53_wait,
            handle: (&mut self.i2c as *mut I2c<'static, Blocking>).cast(),
        };
        let mut device_storage = core::mem::MaybeUninit::<Vl53l8cxConfiguration>::uninit();
        let device_ptr = device_storage.as_mut_ptr();
        unsafe {
            core::ptr::addr_of_mut!((*device_ptr).platform).write(platform);
        }

        let init_status = unsafe { vl53l8cx_init(device_ptr) };
        if init_status != 0 {
            return Err(InitError::DeviceInitFailed { status: init_status });
        }

        let mut device = unsafe { device_storage.assume_init() };

        let resolution_status = unsafe {
            vl53l8cx_set_resolution(&mut device, VL53L8CX_RESOLUTION_8X8)
        };
        if resolution_status != 0 {
            return Err(InitError::SettingResolutionFailed { status: resolution_status })
        }

        self.device_configuration = Some(device);

        return Ok(());
    }

    /// Starts the range meassuring in the lidar
    pub fn start_ranging(&mut self) -> Result<(), StartRangingError>  {
        let mut device_config = self.device_configuration.take()
            .ok_or(StartRangingError::NotInitilized)?;

        let start_status = unsafe { vl53l8cx_start_ranging(&mut device_config) };
        if start_status != 0 {
            self.device_configuration = Some(device_config);
            return Err(StartRangingError::StartRangingFailed { status: start_status });
        }

        self.device_configuration = Some(device_config);
        self.is_running = true;

        return Ok(());
    }

    /// Stops the range meassuring in the lidar
    pub fn stop_ranging(&mut self) -> Result<(), StopRangingError>  {
        let mut device_config = self.device_configuration.take()
            .ok_or(StopRangingError::NotInitilized)?;

        let start_status = unsafe { vl53l8cx_stop_ranging(&mut device_config) };
        if start_status != 0 {
            self.device_configuration = Some(device_config);
            return Err(StopRangingError::StopRangingFailed { status: start_status });
        }

        self.device_configuration = Some(device_config);
        self.is_running = false;

        return Ok(());
    }

    /// Tries to get a reciever for the lidar_reading
    pub fn getting_lidar_reading(&self) -> Option<Receiver<'_, CriticalSectionRawMutex, LidarReading, 4>> {
        return self.lidar_reading.receiver();
    }

    /// Check the lidar for new data. If new data are available the data will be available via getting_lidar_reading
    pub fn check_for_new_data(&mut self) -> Result<bool, CheckForNewDataError> {
        let mut read_results: Vl53l8cxResultsData = unsafe { core::mem::zeroed() };

        let mut device_configuration = match self.device_configuration.take() {
            None => return Err(CheckForNewDataError::NotInitilized),
            Some(config) => config,
        };

        let mut ready = 0;
        let ready_status = unsafe { vl53l8cx_check_data_ready(&mut device_configuration, &mut ready) };

        if ready_status != 0 {
            self.device_configuration = Some(device_configuration);
            return Err(CheckForNewDataError::ReadyError { status: ready_status })
        }

        if ready == 0 {
            self.device_configuration = Some(device_configuration);
            return Ok(false);
        }
        
        let data_status = unsafe {
            vl53l8cx_get_ranging_data(&mut device_configuration, &mut read_results)
        };

        if data_status != 0 {
            self.device_configuration = Some(device_configuration);
            return Err(CheckForNewDataError::DataError { status: data_status });
        }

        self.device_configuration = Some(device_configuration);

        let mut zones: [LidarZone; VL53L8CX_RESOLUTION_8X8_VALUES] = [
            LidarZone {
                ambient_per_spad: 0,
                nb_target_detected: 0,
                nb_spads_enabled: 0,
                signal_per_spad: 0,
                range_sigma_mm: 0,
                distance_mm: 0,
                reflectance: 0,
                target_status: 0,
            };
            VL53L8CX_RESOLUTION_8X8_VALUES
        ];

        for i in 0..VL53L8CX_RESOLUTION_8X8_VALUES {
            zones[i] = LidarZone {
                ambient_per_spad: read_results.ambient_per_spad[i],
                nb_target_detected: read_results.nb_target_detected[i],
                nb_spads_enabled: read_results.nb_spads_enabled[i],
                signal_per_spad: read_results.signal_per_spad[i],
                range_sigma_mm: read_results.range_sigma_mm[i],
                distance_mm: read_results.distance_mm[i],
                reflectance: read_results.reflectance[i],
                target_status: read_results.target_status[i],
            };
        }

        self.lidar_reading.sender().send(
            LidarReading {
                zones: zones,
                silicon_temp_degc: read_results.silicon_temp_degc,
                last_read_timestamp: Instant::now(),
            }
        );

        return Ok(true);
    }
}

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
    /// Global motion indicator value 1 (motion detector output)
    global_indicator_1: u32,
    /// Global motion indicator value 2 (motion detector output)
    global_indicator_2: u32,
    /// Motion detector status; indicates whether motion was detected / valid
    status: u8,
    /// Number of motion aggregates detected in the current frame
    nb_of_detected_aggregates: u8,
    /// Total number of motion aggregates configured/available in the zone
    nb_of_aggregates: u8,
    /// Reserved / spare field
    spare: u8,
    /// Per-zone motion values for the 32 aggregate bins
    motion: [u32; 32],
}

#[repr(C)]
struct Vl53l8cxResultsData {
    /// Internal sensor silicon temperature in °C
    silicon_temp_degc: i8,
    /// Ambient noise in kcps/spad for each of the 64 zones
    ambient_per_spad: [u32; VL53L8CX_RESOLUTION_8X8_VALUES],
    /// Number of valid targets detected for each zone
    nb_target_detected: [u8; VL53L8CX_RESOLUTION_8X8_VALUES],
    /// Number of SPADs enabled for each zone during this ranging
    nb_spads_enabled: [u32; VL53L8CX_RESOLUTION_8X8_VALUES],
    /// Signal returned to the sensor, in kcps/spad, for each zone/target
    signal_per_spad: [u32; VL53L8CX_RESOLUTION_8X8_VALUES],
    /// Sigma of the current distance in mm; lower is better / more stable
    range_sigma_mm: [u16; VL53L8CX_RESOLUTION_8X8_VALUES],
    /// Measured distance in mm for each zone/target
    distance_mm: [i16; VL53L8CX_RESOLUTION_8X8_VALUES],
    /// Estimated reflectance in percent for each zone/target
    reflectance: [u8; VL53L8CX_RESOLUTION_8X8_VALUES],
    /// Status for each target measurement; values 5 and 9 indicate valid ranging
    target_status: [u8; VL53L8CX_RESOLUTION_8X8_VALUES],
    /// Motion detector result data for the current frame
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
    fn vl53l8cx_stop_ranging(device: *mut Vl53l8cxConfiguration) -> u8;
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

        if let Err(_error) = i2c.write(
            VL53L8CX_I2C_ADDRESS,
            &buffer[..2 + chunk_len],
        ) {
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

        if let Err(_error) = i2c.write_read(
            VL53L8CX_I2C_ADDRESS,
            &address_bytes,
            &mut values[offset..offset + chunk_len],
        ) {
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