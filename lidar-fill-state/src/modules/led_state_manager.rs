use core::{f64::consts::PI};

use heapless::Vec;
use embassy_time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemState {
    Booting,
    ConnectingWifi,
    WifiConnected,
    WifiLost,
    WifiUnreachable,
    IpReceived,
    Panic
}
    
impl core::fmt::Display for SystemState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SystemState::Booting => write!(f, "Booting"),
            SystemState::ConnectingWifi => write!(f, "ConnectingWifi"),
            SystemState::WifiConnected => write!(f, "WifiConnected"),
            SystemState::WifiLost => write!(f, "WifiLost"),
            SystemState::WifiUnreachable => write!(f, "WifiUnreachable"),
            SystemState::IpReceived => write!(f, "IpReceived"),
            SystemState::Panic => write!(f, "Panic")
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    WebRequest,
    IoOperation,
    FlashWrite,
}

impl core::fmt::Display for Activity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Activity::WebRequest => write!(f, "WebRequest"),
            Activity::IoOperation => write!(f, "IoOperation"),
            Activity::FlashWrite => write!(f, "FlashWrite")
        }
    }
}

pub enum SetActivityError {
    ActivityListFull
}

struct LedState<TState> {
    state: TState,
    duration_in_ms: Option<u64>,
    timestamp: Option<Instant>
}

pub struct LedStateManager<const ACTIVITY_COUNT: usize> {
    system_state: LedState<SystemState>,
    activities: Vec<LedState<Option<Activity>>, ACTIVITY_COUNT>,
    pub brightness: u8
}

pub struct LedStateAndColor {
    pub color: RgbColor,
    pub system_state: SystemState,
    pub dominant_activity: Option<Activity>
}

struct ActivityAndColor {
    pub color: RgbColor,
    pub dominant_activity: Option<Activity>
}

impl<const ACTIVITY_COUNT: usize> LedStateManager<ACTIVITY_COUNT> {
    /// Initializes the LedStateManager
    pub const fn new() -> Self {
        Self {
            system_state: LedState { state: SystemState::Booting, duration_in_ms: None, timestamp: None },
            activities: Vec::new(),
            brightness: 0xFF
        }
    }

    pub fn set_system_state(&mut self, new_state: SystemState) {
        self.system_state = LedState { state: new_state, duration_in_ms: None, timestamp: Some(Instant::now()) };
    }

    pub fn add_activity(&mut self, activity: Activity, duration_in_ms: Option<u64>) -> Result<(), SetActivityError> {
        match self.activities.push(LedState { state: Some(activity), duration_in_ms: duration_in_ms, timestamp: Some(Instant::now()) }) {
            Ok(()) => Ok(()),
            Err(_) => Err(SetActivityError::ActivityListFull)
        }
    }

    // region: get_active_state_and_color
    pub fn get_active_state_and_color(&mut self) -> LedStateAndColor {
        let timestamp = Instant::now();

        let elapsed_ms = match self.system_state.timestamp {
            Some(value) => timestamp.as_millis() - value.as_millis(),
            None => 0
        };
        const COLOR_TRANSITION_DURATION_MS: usize = 1000;
        let degrees: f64 = elapsed_ms as f64 % COLOR_TRANSITION_DURATION_MS as f64 / COLOR_TRANSITION_DURATION_MS as f64 * 360 as f64;

        let mut i = self.activities.len();
        while i > 0 {
            let activity_timestmap_ms = match self.activities[i - 1].timestamp {
                Some(value) => value.as_millis(),
                None => 0
            };

            if self.activities[i - 1].duration_in_ms != None && 
               timestamp.as_millis() - activity_timestmap_ms > self.activities[i - 1].duration_in_ms.unwrap() {
                self.activities.remove(i - 1);
            }

            i -= 1;
        }

        let mut dominant_activity: Option<Activity> = None;
        let color = match self.system_state.state {
            SystemState::Booting => self.sine_transition_between_colors(
                RgbColor { r: 0xFF, g: 0xFF, b: 0xFF },
                RgbColor { r: 0xAF, g: 0xAF, b: 0xAF },
                degrees
            ),
            SystemState::ConnectingWifi => self.sine_transition_between_colors(
                RgbColor { r: 0xD3, g: 0x00, b: 0xFF },
                RgbColor { r: 0x74, g: 0x00, b: 0xFF },
                degrees
            ),
            SystemState::Panic => self.sine_transition_between_colors(
                RgbColor { r: 0xFF, g: 0x00, b: 0x00 },
                RgbColor { r: 0xFF, g: 0x54, b: 0x00 },
                degrees
            ),
            SystemState::WifiLost => self.sine_transition_between_colors(
                RgbColor { r: 0xFF, g: 0xBF, b: 0x00 },
                RgbColor { r: 0xFF, g: 0x30, b: 0x00 },
                degrees
            ),
            SystemState::WifiUnreachable => self.sine_transition_between_colors(
                RgbColor { r: 0xFF, g: 0xBF, b: 0x00 },
                RgbColor { r: 0x00, g: 0x04, b: 0xFF },
                degrees
            ),
            SystemState::WifiConnected => self.sine_transition_between_colors(
                RgbColor { r: 0x00, g: 0xFF, b: 0x00 },
                RgbColor { r: 0x80, g: 0x00, b: 0xFF },
                degrees
            ),
            SystemState::IpReceived => {
                let activity_and_color= self.get_current_activity_collor(timestamp);
                dominant_activity = activity_and_color.dominant_activity;

                activity_and_color.color
            }
        };

        let color = self.multiply_color_with_brightness(color, self.brightness);

        return LedStateAndColor { color: color, system_state: self.system_state.state, dominant_activity: dominant_activity }
    }
    // endregion

    // region: get_current_activity_collor
    fn get_current_activity_collor(&self, current_timestmap: Instant) -> ActivityAndColor {
        let mut states = [None; ACTIVITY_COUNT];

        let mut i = 0;
        for element in &self.activities {
            states[i] = element.state;

            i += 1;
        }

        const COLOR_TRANSITION_DURATION_MS: usize = 300;

        match index_of_element(&states, &Some(Activity::IoOperation)) {
            None => (),
            Some(index) => {
                let timstamp_ms = match self.activities.get(index).unwrap().timestamp {
                    Some(value) => value.as_millis(),
                    None => 0
                };

                let elapsed_ms = current_timestmap.as_millis() - timstamp_ms;
                let degrees: f64 = elapsed_ms as f64 % COLOR_TRANSITION_DURATION_MS as f64 / COLOR_TRANSITION_DURATION_MS as f64 * 360 as f64;

                let color = self.sine_transition_between_colors(
                    RgbColor { r: 0x00, g: 0xFF, b: 0x00 },
                    RgbColor { r: 0x00, g: 0x00, b: 0xFF },
                    degrees
                );

                return ActivityAndColor { color: color, dominant_activity: Some(Activity::IoOperation) }
            }
        };

        match index_of_element(&states, &Some(Activity::FlashWrite)) {
            None => (),
            Some(index) => {
                let timstamp_ms = match self.activities.get(index).unwrap().timestamp {
                    Some(value) => value.as_millis(),
                    None => 0
                };

                let elapsed_ms = current_timestmap.as_millis() - timstamp_ms;
                let degrees: f64 = elapsed_ms as f64 % COLOR_TRANSITION_DURATION_MS as f64 / COLOR_TRANSITION_DURATION_MS as f64 * 360 as f64;

                let color = self.sine_transition_between_colors(
                    RgbColor { r: 0x00, g: 0xFF, b: 0x00 },
                    RgbColor { r: 0xFF, g: 0x30, b: 0x30 },
                    degrees
                );

                return ActivityAndColor { color: color, dominant_activity: Some(Activity::FlashWrite) }
            }
        };

        match index_of_element(&states, &Some(Activity::WebRequest)) {
            None => (),
            Some(index) => {
                let timstamp_ms = match self.activities.get(index).unwrap().timestamp {
                    Some(value) => value.as_millis(),
                    None => 0
                };

                let elapsed_ms = current_timestmap.as_millis() - timstamp_ms;
                let degrees: f64 = elapsed_ms as f64 % COLOR_TRANSITION_DURATION_MS as f64 / COLOR_TRANSITION_DURATION_MS as f64 * 360 as f64;

                let color = self.sine_transition_between_colors(
                    RgbColor { r: 0x00, g: 0xFF, b: 0x00 },
                    RgbColor { r: 0xFF, g: 0x00, b: 0xFF },
                    degrees
                );

                return ActivityAndColor { color: color, dominant_activity: Some(Activity::WebRequest) }
            }
        };

        return ActivityAndColor { color: RgbColor { r: 0x00, g: 0xFF, b: 0x00 }, dominant_activity: None }
    }
    // endregion

    // region: sine_transition_between_colors
    /// Interpolates between the 2 colors
    /// 
    /// ## Parameters
    /// - color_a: The first color (at 0° and 360°)
    /// - color_b: The second color (at 180°)
    /// - degress: The degrees in the cycle
    /// 
    /// ## Returns
    /// Returns the interpolated color
    fn sine_transition_between_colors(&self, color_a: RgbColor, color_b: RgbColor, degrees: f64) -> RgbColor {
        return RgbColor {
            r: self.sine_interpolate(color_a.r as f64, color_b.r as f64, degrees) as u8,
            g: self.sine_interpolate(color_a.g as f64, color_b.g as f64, degrees) as u8,
            b: self.sine_interpolate(color_a.b as f64, color_b.b as f64, degrees) as u8
        }
    }
    // endregion

    // region: sine_interpolate
    /// Interpolates between two values using a sine wave.
    ///
    /// - 0 degrees   = start_value
    /// - 180 degrees = end_value
    /// - 360 degrees = start_value
    fn sine_interpolate(
        &self,
        start_value: f64,
        end_value: f64,
        degrees: f64,
    ) -> f64 {

        // Keep the angle between 0 and 360 degrees.
        let degrees = degrees % 360.0;

        // Convert degrees to radians.
        let radians = degrees * PI / 180.0;

        // Calculate sine and shift its range from [-1, 1] to [0, 1].
        let progress = (1.0 - libm::cos(radians)) / 2.0;

        // Interpolate between the two values.
        start_value + (end_value - start_value) * progress
    }
    // endregion

    // region: multiply_color_with_brightness
    /// Mutliplies the color with the brightness
    /// 
    /// ## Parameters
    /// - rgb_color: The color to change the color of
    /// - brightness: The brightness to multiply to the color 0 = 0%, 255 = 100%
    /// 
    /// ## Returns
    /// Returns the new color
    fn multiply_color_with_brightness(&self, rgb_color: RgbColor, brightness: u8) -> RgbColor {
        return RgbColor {
            r: ((rgb_color.r as f32 / 256 as f32) * brightness as f32) as u8,
            g: ((rgb_color.g as f32 / 256 as f32) * brightness as f32) as u8,
            b: ((rgb_color.b as f32 / 256 as f32) * brightness as f32) as u8
        }
    }
    // endregion
}

fn index_of_element<TElement: core::cmp::PartialEq, const ARRAY_LENGTH: usize>(array: &[TElement; ARRAY_LENGTH], search_element: &TElement) -> Option<usize> {
    let mut i = 0;
    for element in array {
        if element == search_element {
            return Some(i);
        }

        i += 1;
    }

    return None;
}