use core::sync::atomic::{AtomicBool, Ordering};
use embassy_time::Instant;
use fasttime::{DateTime, UtcOffset, OffsetDateTime};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;

/// This Struct uses the cylces since start from the board togehter with a sync time to
/// give the current real time
pub struct Clock {
    synced: AtomicBool,
    unix_at_sync: Mutex<CriticalSectionRawMutex, u64>,
    ticks_at_sync: Mutex<CriticalSectionRawMutex, u64>,
    utc_offset_at_sync: Mutex<CriticalSectionRawMutex, i64>,
}

impl Clock {
    // region: ctor
    /// Initialize the struct
    pub const fn new() -> Self {
        Self {
            synced: AtomicBool::new(false),
            unix_at_sync: Mutex::new(0),
            ticks_at_sync: Mutex::new(0),
            utc_offset_at_sync: Mutex::new(0),
        }
    }
    // endregion

    // region: set_unix_time
    /// Sets the unix_timestmap in the clock
    /// 
    /// ## Parameters
    /// - unix_seconds: The sconds for the epoch to set
    /// - utc_offset_seconds: The seconds offset from utc
    pub async fn set_unix_time(&self, unix_seconds: u64, utc_offset_seconds: i64) {
        self.synced.store(false, Ordering::SeqCst);
        *self.unix_at_sync.lock().await = unix_seconds * 1000;
        *self.utc_offset_at_sync.lock().await = utc_offset_seconds * 1000;
        *self.ticks_at_sync.lock().await = Instant::now().as_ticks();
        self.synced.store(true, Ordering::SeqCst);
    }
    // endregion

    // region: unix_time
    /// Returns the current Unix timestamp without network access.
    /// 
    /// ## Returns
    /// 
    /// Returns the current timestamp
    pub async fn unix_time(&self) -> Option<OffsetDateTime> {
        if !self.synced.load(Ordering::SeqCst) {
            return None;
        }

        let base_unix_milliseconds = *self.unix_at_sync.lock().await;
        let base_ticks = *self.ticks_at_sync.lock().await;
        let utc_offset_at_sync = *self.utc_offset_at_sync.lock().await;
        let now_ticks = Instant::now().as_ticks();

        let elapsed_ticks = now_ticks.saturating_sub(base_ticks);
        let elapsed_milliseconds = elapsed_ticks / (embassy_time::TICK_HZ / 1000) as u64;

        let current_time = self.clock_timestamp_to_json_timestamp(base_unix_milliseconds + elapsed_milliseconds, utc_offset_at_sync);

        return match current_time {
            Err(_) => None,
            Ok(value) => Some(value)
        };
    }
    // endregion

    // region: clock_timestamp_to_json_timestamp
    /// Converts the unix-timestamp to an iso-date-string
    /// 
    /// ## Parameters
    /// timestamp: The milliseconds since the epoch
    /// utc_offset: The offset to utc
    /// 
    /// ## Returns
    /// Returns the bytes of the iso-date-string
    pub fn clock_timestamp_to_json_timestamp(
        &self,
        timestamp: u64,
        utc_offset: i64,
    ) -> Result<OffsetDateTime, &'static str> {

        let seconds = (timestamp / 1000) as i64;
        let millis = (timestamp % 1000) as i32;

        let dt = DateTime::from_unix_timestamp(
            seconds,
            millis * 1_000_000,
        ).map_err(|_| "Invalid timestamp")?;

        let offset = UtcOffset::from_seconds(
            utc_offset as i32
        ).map_err(|_| "Invalid UTC offset")?;

        Ok(OffsetDateTime::from_utc(dt, offset))
    }
    // endregion
}

/// The main clock
pub static CLOCK: Clock = Clock::new();