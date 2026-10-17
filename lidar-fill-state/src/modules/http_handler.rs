use core::fmt::{Debug, Display, Write as FmtWrite};
use alloc::string::{String, ToString};
use edge_http::{
    io::{
        server::{
            Connection,
            Handler,
        }
    },
    Method,
};
use heapless::CapacityError;
use log::{debug, error, info, trace, warn};
use embedded_io_async::{Read, Write};
use esp_println as _;
use littlefs_rust::{
    Error, Filesystem, OpenFlags
};
use embassy_sync::{
    blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex}, mutex::{Mutex, MutexGuard},
};
use embassy_time::{Duration, with_timeout};
use gzip_bytes::gzip_bytes;

pub enum WriteFileFromRequestError<E> {
    Fs(Error),
    Http(edge_http::io::Error<E>),
}

impl<E> From<Error> for WriteFileFromRequestError<E> {
    fn from(error: Error) -> Self {
        Self::Fs(error)
    }
}

impl<E> From<edge_http::io::Error<E>> for WriteFileFromRequestError<E> {
    fn from(error: edge_http::io::Error<E>) -> Self {
        Self::Http(error)
    }
}

/// The html of the index page of the webserver
const HTTP_INDEX_HTML: &'static [u8] = gzip_bytes!("../../../web/index.html");
/// The html of the settings page of the webserver
const HTTP_SETTINGS_HTML: &'static [u8] = gzip_bytes!("../../../web/settings.html");
/// The base css of the web pages of the webserver
const HTTP_BASE_STYLE_CSS: &'static [u8] = gzip_bytes!("../../../web/assets/base_style.css");
/// The base css of the web pages of the webserver
const HTTP_COG_ICON: &'static [u8] = gzip_bytes!("../../../web/assets/cog_icon.svg");
/// The Fav-Icon of the website
const HTTP_FAV_ICON: &'static [u8] = gzip_bytes!("../../../web/favicon.ico");
/// The html of the swagger page
const HTTP_SWAGGER_HTML: &'static [u8] = gzip_bytes!("../../../web/swagger/index.html");
/// The openapi json for the this api
const HTTP_OPENAPI_JSON: &'static [u8] = gzip_bytes!("../../../web/swagger/openapi.json");
/// The html of the settings saved page
const HTTP_SETTINGS_SAVE_SUCCESS_HTML: &'static [u8] = gzip_bytes!("../../../web/settings_success.html");
/// The Path of the 3D-Model in the filesystem
const GLB_FILE_PATH: &str = "Model.glb";
/// The Path of the Settings-File in the filesystem
const SETTINGS_JSON_FILE_PATH: &str = "Settings.json";
/// The size of a storage block
const STORAGE_BLOCK_SIZE: u32 = 4 * 1_024; // TODO: Mit main.rs const verbinden
/// The amount of blocks in the storage
const STORAGE_BLOCK_COUNT: u32 = 528; // TODO: Mit main.rs const verbinden
/// The size of the http-connection buffer
const HTTP_CONNECTION_BUFFER_SIZE: usize = 1024;
/// The maximum size of a json response
const HTTP_JSON_REQUEST_MAX_SIZE: usize = 1024;

use crate::modules::{
    file_manager::FileManager, html_form::Form, little_fs_storage::LittleFsStorage, vl53l8cx_lidar_service::LidarReadingWatch,
};

pub type LidarJsonResponseBuffer =
    Mutex<CriticalSectionRawMutex, heapless::String<16_384>>;

pub struct HttpHandler {
    pub filesystem: &'static Mutex<
        NoopRawMutex,
        Filesystem<LittleFsStorage<'static>>,
    >,
    pub file_manager: &'static Mutex<
        NoopRawMutex,
        FileManager,
    >,
    pub reading_watch: &'static LidarReadingWatch,
    pub json_response_buffer: &'static LidarJsonResponseBuffer,
}

impl HttpHandler {
    /// Saves the Settings in the file system
    /// 
    /// ## Return codes
    /// - 200: The Settings was saved
    async fn handle_post_settings<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        info!("handle_post_settings > Was called...");

        debug!("handle_post_settings > calling save_settings_from_connection");
        match self.save_settings_from_connection(conn).await {
            Ok(()) => {},
            Err(error) => return self.handle_save_settings_error(conn, error).await
        }
        debug!("handle_post_settings > Settings were saved.");

        debug!("handle_post_settings > Initialising the response...");
        match conn.initiate_response(
            303,
            Some("See Other"),
            &[("Location", "/success")],
        ).await {
            Ok(()) => {},
            Err(error) => {
                error!("handle_post_settings > Error while initializing the response: {}", error);
                return Err(error);
            }
        };
        trace!("handle_post_settings > Response was initialized. Request completed.");

        return Ok(())
    }

    /// Get the active 3D-Model file.
    /// 
    /// ## Return codes
    /// - 200: The file was found and returned
    /// - 404: The file was not found
    async fn handle_get_model3d<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        info!("handle_get_model3d > Was called...");

        debug!("handle_get_model3d > Waiting for the file system lock...");
        let filemanager = self.file_manager.lock().await;
        trace!("handle_get_model3d > File system lock was aquired.");

        debug!("handle_get_model3d > Getting the size of the 3d-model...");
        let size = match filemanager.get_3dmodel_size::<HTTP_CONNECTION_BUFFER_SIZE>().await {
            Ok(size) => size,
            Err(error) => {
                error!("handle_get_model3d > Error while getting the file size: {}", error);
                return self.setup_500_code_on_connection(conn, None).await;
            }
        };
        trace!("handle_get_model3d > File size was received ({}).", size);

        let mut content_length = heapless::String::<10>::new();
        write!(content_length, "{}", size).unwrap();
        let mut content_disposition = heapless::String::<64>::new();
        write!(content_disposition, "attachment; filename=\"{}\"", GLB_FILE_PATH).unwrap();

        debug!("handle_get_model3d > Initialising the response...");
        match conn.initiate_response(
            200,
            Some("OK"),
            &[
                ("Content-Type", "model/gltf-binary"),
                ("Content-Length", content_length.as_str()),
                ("Content-Disposition", content_disposition.as_str())
            ],
        ).await {
            Ok(()) => {},
            Err(error) => {
                error!("handle_get_model3d > Error while initializing the response: {}", error);
                return Err(error);
            }
        };
        trace!("handle_get_model3d > Response was initialized.");

        debug!("handle_get_model3d > Loading the 3d-model from the file system...");
        match filemanager.load_3dmodel::<_, HTTP_CONNECTION_BUFFER_SIZE>(conn).await {
            Ok(()) => {},
            Err(error) => {
                match error {
                    crate::modules::file_manager::FileManagerError::Fs(error) => {
                        error!("handle_get_model3d > Error while loading the 3d-model file: {}", error);
                        return self.setup_500_code_on_connection(conn, None).await;
                    },
                    crate::modules::file_manager::FileManagerError::Generic(error) => {
                        error!("handle_get_model3d > Error while loading the 3d-model file: {}", error);
                        return Err(error);
                    }
                }
            }
        };
        trace!("handle_get_model3d > 3d-model was loaded. Request completed.");

        return Ok(());
    }

    /// Sets the active 3D-Model file
    /// 
    /// ## Parameters
    /// - Body: The glb file that should be set
    /// 
    /// ## Return codes
    /// - 204: The file was saved
    async fn handle_put_model3d<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        info!("handle_put_model3d > Was called...");

        debug!("handle_get_model3d > Waiting for the file system lock...");
        let filemanager = self.file_manager.lock().await;
        trace!("handle_get_model3d > File system lock was aquired.");

        debug!("handle_put_model3d > Calling save_3dmodel...");
        match filemanager.save_3dmodel::<_, HTTP_CONNECTION_BUFFER_SIZE>(conn).await {
            Ok(()) => {},
            Err(error) => {
                match error {
                    crate::modules::file_manager::FileManagerError::Fs(error) => {
                        error!("Could not open file {} for writing: {:?}", GLB_FILE_PATH, error);
                    }
                    crate::modules::file_manager::FileManagerError::Generic(error) => {
                        error!("Could not read from the request: {:?}", error);
                    }
                }

                return self.setup_500_code_on_connection(conn, None).await;
            }
        };
        trace!("handle_put_model3d > Completed save_3dmodel.");

        debug!("handle_put_model3d > Initialising the response...");
        match conn.initiate_response(
            204,
            Some("No Content"),
            &[],
        ).await {
            Ok(()) => {},
            Err(error) => {
                error!("handle_put_model3d > Error while initializing the response: {}", error);
                return Err(error);
            }
        };
        trace!("handle_put_model3d > Response was initialized. Request completed.");

        return Ok(())
    }

    /// Get the settings
    /// 
    /// ## Return codes
    /// - 200: The settings were received
    /// - 404: Settings not found
    async fn handle_api_get_settings<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        info!("handle_api_get_settings > Was called...");

        debug!("handle_api_get_settings > Waitung for the file system lock...");
        let filemanager = self.file_manager.lock().await;
        trace!("handle_api_get_settings > File system lock was aquired.");

        let mut content_disposition = heapless::String::<64>::new();
        write!(content_disposition, "attachment; filename=\"{}\"", SETTINGS_JSON_FILE_PATH).unwrap();

        debug!("handle_api_get_settings > Initialising the response...");
        match conn.initiate_response(
            200,
            Some("OK"),
            &[
                ("Content-Type", "application/json"),
                ("Content-Disposition", content_disposition.as_str())
            ],
        ).await {
            Ok(()) => {},
            Err(error) => {
                error!("handle_api_get_settings > Error while initializing the response: {}", error);
                return Err(error);
            }
        };
        trace!("handle_api_get_settings > Response was initialized.");

        debug!("handle_api_get_settings > Loads the settings from the fs...");
        match filemanager.load_settings::<_, HTTP_CONNECTION_BUFFER_SIZE>(conn).await {
            Ok(()) => {},
            Err(error) => {
                match error {
                    crate::modules::file_manager::FileManagerError::Fs(error) => {
                        // TODO: Hier muss 404 extra zurück gegeben
                        error!("handle_api_get_settings > Error while loading the settings file: {}", error);
                        return self.setup_500_code_on_connection(conn, None).await;
                    },
                    crate::modules::file_manager::FileManagerError::Generic(error) => {
                        error!("handle_api_get_settings > Error while loading the settings file: {}", error);
                        return Err(error);
                    }
                }
            }
        };
        trace!("handle_api_get_settings > File was loaded. Request completed.");

        return Ok(());
    }

    /// Saves the settings in the fs
    /// 
    /// ## Return codes
    /// - 204: The settings were saved
    /// - 400: The Request-Parametrs were invalid
    async fn handle_put_settings<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        info!("handle_put_settings > Was called...");

        debug!("handle_put_settings > calling save_settings_from_connection");
        match self.save_settings_from_connection(conn).await {
            Ok(()) => {},
            Err(error) => return self.handle_save_settings_error(conn, error).await
        }
        debug!("handle_put_settings > Settings were saved. Reqeust completed.");

        debug!("handle_put_settings > Initialising the response...");
        match conn.initiate_response(
            204,
            Some("No Content"),
            &[],
        ).await {
            Ok(()) => {},
            Err(error) => {
                error!("handle_put_settings > Error while initializing the response: {}", error);
                return Err(error);
            }
        };
        trace!("handle_put_settings > Response was initialized. Request completed.");

        return Ok(());
    }

    /// Returns the latest LIDAR data as JSON
    /// 
    /// ## Returns codes
    /// - 200: The reading was successfully received
    async fn handle_get_api_data_full<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        info!("handle_get_api_data_full > Was called...");

        debug!("handle_get_api_data_full > Getting the latest lidar reading...");
        let mut receiver = self.reading_watch.receiver().unwrap();
        let lidar_reading = receiver.get().await;
        trace!("handle_get_api_data_full > Lidar reading was received.");

        debug!("handle_get_api_data_full > Parsing the lidar reading to json...");
        let json = nojson::json(|f| {
            f.object(|f| {
                f.member("silicon_temp_degc", lidar_reading.silicon_temp_degc)?;
                f.member(
                    "last_read_timestamp_ms",
                    lidar_reading.last_read_timestamp.as_millis(),
                )?;
                f.member("zones", nojson::json(|f| {
                    f.array(|f| {
                        for zone in &lidar_reading.zones {
                            f.element(nojson::json(|f| {
                                f.object(|f| {
                                    f.member("ambient_per_spad", zone.ambient_per_spad)?;
                                    f.member("nb_target_detected", zone.nb_target_detected)?;
                                    f.member("nb_spads_enabled", zone.nb_spads_enabled)?;
                                    f.member("signal_per_spad", zone.signal_per_spad)?;
                                    f.member("range_sigma_mm", zone.range_sigma_mm)?;
                                    f.member("distance_mm", zone.distance_mm)?;
                                    f.member("reflectance", zone.reflectance)?;
                                    f.member("target_status", zone.target_status)
                                })
                            }))?;
                        }
                        Ok(())
                    })
                }))
            })
        });
        trace!("handle_get_api_data_full > Parsing to json done.");

        debug!("handle_get_api_data_full > Initialising the response...");
        match conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "application/json")],
        ).await {
            Ok(()) => {},
            Err(error) => {
                error!("handle_get_api_data_full > Error while initializing the response: {}", error);
                return Err(error);
            }
        };
        trace!("handle_get_api_data_full > Response was initialized.");

        debug!("handle_get_api_data_full > Writing the json to the response...");
        let mut response = self.json_response_buffer.lock().await;
        response.clear();
        write!(response, "{}", json).unwrap();
        match conn.write_all(response.as_bytes()).await {
            Ok(()) => {},
            Err(error) => {
                error!("handle_post_settings > Error while writing the response body: {}", error);
                return Err(error);
            }
        };
        trace!("handle_get_api_data_full > Text was written. Request completed.");

        return Ok(())
    }

    /// Returns the latest LIDAR data as JSON
    /// 
    /// ## Returns codes
    /// - 200: The reading was successfully received
    async fn handle_get_api_data<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        info!("handle_get_api_data > Was called...");

        debug!("handle_get_api_data > Getting the latest lidar reading...");
        let mut receiver = self.reading_watch.receiver().unwrap();
        let lidar_reading = receiver.get().await;
        trace!("handle_get_api_data > Lidar reading was received.");

        let mut distances: [i16; 64] = [0; 64];

        for i in 0..64 {
            distances[i] = lidar_reading.zones[i].distance_mm;
        }

        debug!("handle_get_api_data > Parsing the lidar reading to json...");
        let json = nojson::json(|f| {
            f.object(|f| {
                f.member("silicon_temp_degc", lidar_reading.silicon_temp_degc)?;
                f.member(
                    "last_read_timestamp_ms",
                    lidar_reading.last_read_timestamp.as_millis(),
                )?;
                f.member("distance_mm", distances)
            })
        });
        trace!("handle_get_api_data > Parsing to json done.");

        debug!("handle_get_api_data > Initialising the response...");
        match conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "application/json")],
        ).await {
            Ok(()) => {},
            Err(error) => {
                error!("handle_get_api_data > Error while initializing the response: {}", error);
                return Err(error);
            }
        };
        trace!("handle_get_api_data > Response was initialized.");

        debug!("handle_get_api_data > Writing the json to the response...");
        let mut response = self.json_response_buffer.lock().await;
        response.clear();
        write!(response, "{}", json).unwrap();
        match conn.write_all(response.as_bytes()).await {
            Ok(()) => {},
            Err(error) => {
                error!("handle_get_api_data > Error while writing the response body: {}", error);
                return Err(error);
            }
        };
        trace!("handle_get_api_data > Text was written. Request completed.");

        return Ok(());
    }

    ///⚠️ Returns the latest LIDAR data as JSON
    /// 
    /// ## Returns codes
    /// - 200: The reading was successfully received
    async fn handle_get_fillstate<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        info!("handle_get_fillstate > Was called...");

        debug!("handle_get_fillstate > Getting the latest lidar reading...");
        let mut receiver = self.reading_watch.receiver().unwrap();
        let lidar_reading = receiver.get().await;
        trace!("handle_get_fillstate > Lidar reading was received.");

        let mut distances: [i16; 64] = [0; 64];

        let fs = self.filesystem.lock().await;
        
        // TODO: Programmmieren






        for i in 0..64 {
            distances[i] = lidar_reading.zones[i].distance_mm;
        }

        debug!("handle_get_fillstate > Parsing the lidar reading to json...");
        let json = nojson::json(|f| {
            f.object(|f| {
                f.member("silicon_temp_degc", lidar_reading.silicon_temp_degc)?;
                f.member(
                    "last_read_timestamp_ms",
                    lidar_reading.last_read_timestamp.as_millis(),
                )?;
                f.member("distance_mm", distances)
            })
        });
        trace!("handle_get_fillstate > Parsing to json done.");

        debug!("handle_get_fillstate > Initialising the response...");
        match conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "application/json")],
        ).await {
            Ok(()) => {},
            Err(error) => {
                error!("handle_get_fillstate > Error while initializing the response: {}", error);
                return Err(error);
            }
        };
        trace!("handle_get_fillstate > Response was initialized.");

        debug!("handle_get_fillstate > Writing the json to the response...");
        let mut response = self.json_response_buffer.lock().await;
        response.clear();
        write!(response, "{}", json).unwrap();
        match conn.write_all(response.as_bytes()).await {
            Ok(()) => {},
            Err(error) => {
                error!("handle_get_fillstate > Error while writing the response body: {}", error);
                return Err(error);
            }
        };
        trace!("handle_get_fillstate > Text was written. Request completed.");

        return Ok(());
    }

    /// Get the stats of the storage of the esp32
    /// 
    /// ## Return codes
    /// - 200: Returns the stats of the file system
    async fn handle_get_api_storage_info<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        info!("handle_get_api_storage_info > Was called...");

        debug!("handle_get_api_storage_info > Waiting for a lock on the file system...");
        let fs = self.filesystem.lock().await;
        trace!("handle_get_api_storage_info > Filesystem lock was aquired.");

        debug!("handle_get_api_storage_info > Calling get_fs_stat...");
        let file_system_stat = match get_fs_stat(fs) {
            Ok(stat) => stat,
            Err(error) => {
                error!("handle_get_api_storage_info > Error while getting the fs stats: {:?}.", error);
                return self.setup_500_code_on_connection(conn, None).await;
            }
        };
        trace!("handle_get_api_storage_info > Completed get_fs_stat.");

        debug!("handle_get_api_storage_info > Parsing the json for the response...");
        let json = nojson::json(|f| {
            f.object(|f| {
                f.member("total_bytes", file_system_stat.total_bytes)?;
                f.member("used_bytes", file_system_stat.used_bytes)?;
                f.member("free_bytes", file_system_stat.free_bytes)
            })
        });
        trace!("handle_get_api_storage_info > Json parse is complete.");

        debug!("handle_get_api_storage_info > Initialising the response...");
        match conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "application/json")],
        ).await {
            Ok(()) => {},
            Err(error) => {
                error!("handle_get_api_storage_info > Error while initializing the response: {}", error);
                return Err(error);
            }
        };
        trace!("handle_get_api_storage_info > Response was initialized.");

        let mut response = self.json_response_buffer.lock().await;
        response.clear();
        write!(response, "{}", json).unwrap();

        debug!("handle_get_api_storage_info > Writing the json to the response...");
        match conn.write_all(response.as_bytes()).await {
            Ok(()) => {},
            Err(error) => {
                error!("handle_get_api_storage_info > Error while writing the response body: {}", error);
                return Err(error);
            }
        };
        trace!("handle_get_api_storage_info > Json was written. Request completed.");

        return Ok(())
    }

    /// Saves the settings json from the connection in the fs
    async fn save_settings_from_connection<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), SaveSettingsError<T>>
    where
        T: Read + Write,
    {
        info!("save_settings_from_connection > Was called...");

        debug!("save_settings_from_connection > Validating the \"Content-Type\" Header...");
        let content_type_header: Option<heapless::String<64>> = {
            let header = match conn.headers() {
                Err(error) => return Err(SaveSettingsError::HttpError { error }),
                Ok(header) => header
            };
            
            header.headers.get("Content-Type").and_then(|value| {
                let mut owned = heapless::String::new();
                owned.push_str(value).ok()?;
                Some(owned)
            })
        };

        let content_type_header = content_type_header.as_deref();

        let settings: Settings;

        if content_type_header == Some("application/json") {
            debug!("save_settings_from_connection > Parsing the json from the request...");
            let parse_result = parse_json_from_reader::<HTTP_JSON_REQUEST_MAX_SIZE, _, _, _>(
                conn,
                parse_settings_from_json,
            ).await;

            settings = match parse_result {
                Ok(settings) => settings,
                Err(JsonParseError::BodyTooLong) => {
                    return Err(SaveSettingsError::BodyTooLong { max_length: HTTP_JSON_REQUEST_MAX_SIZE })
                }
                Err(error) => {
                    return Err(SaveSettingsError::JsonParseError { error })
                }
            };
            trace!("save_settings_from_connection > The settings were parsed.");
        }
        else if content_type_header == Some("application/x-www-form-urlencoded") {
            let parse_result = parse_www__form_urlencoded_from_reader::<HTTP_JSON_REQUEST_MAX_SIZE, _, _, _>(
                conn,
                parse_settings_from_www_form,
            ).await;

            settings = match parse_result {
                Ok(settings) => settings,
                Err(WwwFormParseError::BodyTooLong) => {
                    return Err(SaveSettingsError::BodyTooLong { max_length: HTTP_JSON_REQUEST_MAX_SIZE })
                }
                Err(WwwFormParseError::ReaderReadFailed) => {
                    return Err(SaveSettingsError::JsonParseError { error: JsonParseError::ReaderReadFailed })
                }
                Err(WwwFormParseError::Utf8ConvertError { error }) => {
                    return Err(SaveSettingsError::JsonParseError { error: JsonParseError::Utf8ConvertError { error } })
                }
                Err(WwwFormParseError::JsonParseError { error }) => {
                    return Err(SaveSettingsError::JsonParseError { error: JsonParseError::JsonParseError { error } })
                }
            };
            trace!("save_settings_from_connection > The settings were parsed from the form body.");
        }
        else {
            let mut expected_type = heapless::String::new();
            match expected_type.push_str("application/json") {
                Err(error) => return Err(SaveSettingsError::ContentCapacityError { error }),
                Ok(()) => {}
            };

            let mut acutal_type = heapless::String::new();
            match acutal_type.push_str(content_type_header.unwrap_or("<Not-Found>")) {
                Err(error) => return Err(SaveSettingsError::ContentCapacityError { error }),
                Ok(()) => {}
            };

            return Err(
                SaveSettingsError::ContentTypeWrong {
                    expected_type: expected_type,
                    acutal_type: acutal_type
                }
            );
        }
        trace!("save_settings_from_connection > The \"Content-Type\" Header was correct.");

        debug!("save_settings_from_connection > Parsing settings to json...");
        let json = nojson::json(|f| {
            f.object(|f| {
                f.member("main_mesh_name", settings.main_mesh_name.as_str())?;
                f.member("lidar_camera_name", settings.lidar_camera_name.as_str())?;
                f.member("wifi_ssid", settings.wifi_ssid.as_str())?;
                f.member("wifi_password", settings.wifi_password.as_str())?;

                Ok(())
            })
        });
        trace!("save_settings_from_connection > Parsing to json done.");

        let path = "settings.json";
        debug!("save_settings_from_connection > Waiting for a lock on the file system...");
        let fs = self.filesystem.lock().await;
        trace!("save_settings_from_connection > Filesystem lock was aquired.");

        debug!("save_settings_from_connection > Trying to open the file \"{}\" ...", path);
        let file: littlefs_rust::File<'_, LittleFsStorage<'static>> = match fs.open(path, OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNC) {
            Ok(file) => file,
            Err(error) => {
                return Err(SaveSettingsError::OpenFileError { error })
            }
        };
        trace!("save_settings_from_connection > File was successfully opend.");

        debug!("save_settings_from_connection > Writing the json to the file \"{}\" ...", path);
        match file.write(json.to_string().as_bytes()) {
            Ok(_) => {},
            Err(error) => {
                return Err(SaveSettingsError::WriteFileError { error })
            }
        };
        trace!("save_settings_from_connection > Writing is done.");

        return Ok(());
    }

    /// Returns the error-page, when an unknown path was requested
    /// 
    /// ## Returns codes
    /// - 404: Will always be returned
    async fn handle_not_found<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        info!("handle_not_found > Was called...");

        debug!("handle_not_found > Initialising the response...");
        match conn.initiate_response(
            404,
            Some("Not Found"),
            &[("Content-Type", "text/plain")],
        ).await {
            Ok(()) => {},
            Err(error) => {
                error!("handle_not_found > Error while initializing the response: {}", error);
                return Err(error);
            }
        };
        trace!("handle_not_found > Response was initialized.");

        debug!("handle_not_found > Writing the text to the response...");
        match conn.write_all(b"404 Not Found\n").await {
            Ok(()) => {},
            Err(error) => {
                error!("handle_not_found > Error while writing the response body: {}", error);
                return Err(error);
            }
        };
        trace!("handle_not_found > Text was written. Request completed.");

        return Ok(())
    }

    /// Serves the static content
    /// 
    /// ## Return codes
    /// - 200: The static content was found and returned
    async fn serve_static_content<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
        mime_type: &str,
        content_bytes: &[u8]
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        info!("serve_static_content > Was called...");

        let mut content_length = heapless::String::<10>::new();
        write!(content_length, "{}", content_bytes.len()).unwrap();

        debug!("serve_static_content > Initialising the response (200; {})...", mime_type);
        match conn.initiate_response(
            200,
            Some("OK"),
            &[
                ("Content-Type", mime_type),
                ("Content-Length", content_length.as_str()),
                ("Content-Encoding", "gzip"),
            ],
        ).await {
            Ok(()) => {},
            Err(error) => {
                error!("serve_static_content > Error while initializing the response: {}", error);
                return Err(error);
            }
        };
        trace!("serve_static_content > Response was initialized.");

        debug!("serve_static_content > Writing the bytes ({}) to the response...", content_bytes.len());
        match conn.write_all(content_bytes).await {
            Ok(()) => {},
            Err(error) => {
                error!("serve_static_content > Error while writing the response body: {}", error);
                return Err(error);
            }
        };
        trace!("serve_static_content > Bytes was written. Request completed.");

        return Ok(());
    }

    /// Logs and returns the appropriete Error to the web client
    /// 
    /// ## Parameters
    /// - conn: The connection of the request
    /// - save_error: The error that should be handled
    /// 
    /// ## Return codes
    /// - 400: The settigs was invalid
    async fn handle_save_settings_error<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
        save_error: SaveSettingsError<T>
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        debug!("handle_save_settings_error > Was called...");

        match save_error {
            SaveSettingsError::HttpError { error } => {
                error!("handle_put_settings > Unkown Error while getting the headers of the request: {}", error);
                return Err(error)
            },
            SaveSettingsError::ContentCapacityError { error } => {
                error!("handle_put_settings > The Content-Type header couldn't be parsed because it was to long: {}", error);
                return self.setup_500_code_on_connection(conn, None).await;
            },
            SaveSettingsError::ContentTypeWrong { expected_type, acutal_type } => {
                error!(
                    "handle_put_settings > Error while saving the settings. The received Content-Type was unexpeted (actual: {}; received: {}). Sending 400 back...",
                    acutal_type,
                    expected_type
                );
                return self.setup_400_code_wrong_content_type(
                    conn,
                    expected_type.as_str(),
                    Some(acutal_type.as_str())
                ).await;
            },
            SaveSettingsError::BodyTooLong { max_length } => {
                error!(
                    "handle_put_settings > Error while saving the settings. The received body was longer than the max length ({}). Sending 400 back...",
                    max_length
                );
                return self.setup_400_code_body_too_long(
                    conn, 
                    HTTP_JSON_REQUEST_MAX_SIZE
                ).await;
            },
            SaveSettingsError::JsonParseError { error } => match error {
                JsonParseError::BodyTooLong => {
                    error!("handle_put_settings > Error while parsing the json to save (BodyTooLong).");
                    return self.setup_400_code_json_inavalid(conn).await;
                },
                JsonParseError::ReaderReadFailed => {
                    error!("handle_put_settings > Error while parsing the json to save (ReaderReadFailed).");
                    return self.setup_500_code_on_connection(conn, None).await;
                },
                JsonParseError::Utf8ConvertError { error } => {
                    error!("handle_put_settings > Error while parsing the json to save (Utf8ConvertError).{}", error);
                    return self.setup_500_code_on_connection(conn, None).await;
                },
                JsonParseError::JsonParseError { error } => {
                    error!("handle_put_settings > Error while parsing the json to save (JsonParseError): {}", error);
                    return self.setup_500_code_on_connection(conn, None).await;
                }
            },
            SaveSettingsError::OpenFileError { error } => {
                error!("handle_put_settings > Error while opening the file to save the settings to: {}", error);
                return self.setup_500_code_on_connection(conn, None).await;
            },
            SaveSettingsError::WriteFileError { error } => {
                error!("handle_put_settings > Error while writing the settings to the file: {}", error);
                return self.setup_500_code_on_connection(conn, None).await;
            }
        }
    }

    /// Sets the http code 500 on the connection
    /// 
    /// ## Parameters
    /// - conn: The connection to write the code to
    /// - body_text: The text that should be written to the body
    async fn setup_500_code_on_connection<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
        body_text: Option<&str>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        debug!("setup_500_code_on_connection > Was called...");

        debug!("setup_500_code_on_connection > Initialising the response...");
        match conn.initiate_response(
            500,
            Some("Internal Server Error"),
            &[("Content-Type", "text/plain")],
        ).await {
            Ok(()) => {},
            Err(error) => {
                error!("setup_500_code_on_connection > Error while initializing the response: {}", error);
                return Err(error);
            }
        };
        trace!("setup_500_code_on_connection > Response was initialized.");

        match body_text {
            None => {},
            Some(body_text) => {
                debug!("setup_500_code_on_connection > Writing the text to the response...");
                match conn.write_all(body_text.as_bytes()).await {
                    Ok(()) => {},
                    Err(error) => {
                        error!("setup_500_code_on_connection > Error while writing the response body: {}", error);
                        return Err(error);
                    }
                };
            }
        }

        trace!("setup_500_code_on_connection > Is Done.");

        return Ok(());
    }

    /// Setts the http code 400 on the connection and a description
    /// for the wrong content-type
    /// 
    /// ## Parameters 
    /// - conn: The connection to write the code to
    /// - expected_content_type_header: The expected content type
    /// - actual_content_type_header: The actual content type
    async fn setup_400_code_wrong_content_type<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
        expected_content_type_header: &str,
        actual_content_type_header: Option<&str>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        debug!("setup_400_code_wrong_content_type > Was called...");

        debug!("setup_400_code_wrong_content_type > \"Content-Type\" Header is wrong. Initialising 400...");
        match conn.initiate_response(
            400,
            Some("Bad Request"),
            &[("Content-Type", "application/json")],
        ).await {
            Ok(()) => {},
            Err(error) => {
                error!("setup_400_code_wrong_content_type > Error while initializing the response: {}", error);
                return Err(error);
            }
        };
        trace!("setup_400_code_wrong_content_type > Response was initialized.");

        debug!("setup_400_code_wrong_content_type > Writing the json to the response...");
        let mut response = self.json_response_buffer.lock().await;
        response.clear();
        write!(
            response,
            "{{ header.content-type: \"Content-Type was unexpected (expected: \\\"{}\\\"; actual: \\\"{}\\\"\" }}",
            expected_content_type_header,
            actual_content_type_header.unwrap_or("<Missing>")
        ).unwrap();
        match conn.write_all(response.as_bytes()).await {
            Ok(()) => {},
            Err(error) => {
                error!("setup_400_code_wrong_content_type > Error while writing the response body: {}", error);
                return Err(error);
            }
        };
        trace!("setup_400_code_wrong_content_type > Text was written. Request completed.");

        return Ok(());
    }

    /// Setts the http code 400 on the connection and a description
    /// for the body that is too long
    /// 
    /// ## Parameters 
    /// - conn: The connection to write the code to
    /// - max_length: The max length for the body
    async fn setup_400_code_body_too_long<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
        max_length: usize,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        debug!("setup_400_code_body_too_long > Was called...");

        debug!("setup_400_code_body_too_long > Body is too long. Initialising 400...");
        match conn.initiate_response(
            400,
            Some("Bad Request"),
            &[("Content-Type", "application/json")],
        ).await {
            Ok(()) => {},
            Err(error) => {
                error!("setup_400_code_body_too_long > Error while initializing the response: {}", error);
                return Err(error);
            }
        };
        trace!("setup_400_code_body_too_long > Response was initialized.");

        debug!("setup_400_code_body_too_long > Writing the json to the response...");
        let mut response = self.json_response_buffer.lock().await;
        response.clear();
        write!(
            response,
            "{{ body.too_long: \"The body is longer then {}\" }}",
            max_length
        ).unwrap();
        match conn.write_all(response.as_bytes()).await {
            Ok(()) => {},
            Err(error) => {
                error!("setup_400_code_body_too_long > Error while writing the response body: {}", error);
                return Err(error);
            }
        };
        trace!("setup_400_code_body_too_long > Text was written. Request completed.");

        return Ok(());
    }

    /// Setts the http code 400 on the connection and a description
    /// for the body that the json was wrong
    /// 
    /// ## Parameters 
    /// - conn: The connection to write the code to
    async fn setup_400_code_json_inavalid<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        debug!("setup_400_code_json_inavalid > Was called...");

        debug!("setup_400_code_json_inavalid > Body is too long. Initialising 400...");
        match conn.initiate_response(
            400,
            Some("Bad Request"),
            &[("Content-Type", "application/json")],
        ).await {
            Ok(()) => {},
            Err(error) => {
                error!("setup_400_code_json_inavalid > Error while initializing the response: {}", error);
                return Err(error);
            }
        };
        trace!("setup_400_code_json_inavalid > Response was initialized.");

        debug!("setup_400_code_json_inavalid > Writing the json to the response...");
        let mut response = self.json_response_buffer.lock().await;
        response.clear();
        write!(
            response,
            "{{ body.json_invalid: \"The json in the body is invalid.\" }}"
        ).unwrap();
        match conn.write_all(response.as_bytes()).await {
            Ok(()) => {},
            Err(error) => {
                error!("setup_400_code_json_inavalid > Error while writing the response body: {}", error);
                return Err(error);
            }
        };
        trace!("setup_400_code_json_inavalid > Text was written. Request completed.");

        return Ok(());
    }

    /// Setts the http code 500 on the connection and log that
    /// a timeout happend
    /// 
    /// ## Parameters 
    /// - conn: The connection to write the code to
    async fn setup_500_request_timeout<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
        method_type: &str,
        path: &str,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        debug!("setup_500_request_timeout > Was called...");

        warn!("setup_500_request_timeout > Request \"{} {}\" timed out. Returning HTTP 504...", method_type, path);
        conn.initiate_response(
            504,
            Some("Gateway Timeout"),
            &[
                ("Content-Type", "text/plain; charset=utf-8"),
                ("Content-Length", "18"),
            ],
        ).await?;
        conn.write_all(b"Request timed out\n").await?;
        trace!("setup_500_request_timeout > Timeout response was sent.");

        return Ok(());
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
        // Copy the routing information out of the borrowed headers.
        let (method, path) = {
            let headers = conn.headers()?;

            let method = headers.method;
            let mut path: heapless::String<128> = heapless::String::new();

            path.push_str(headers.path).unwrap();

            (method, path)
        };

        trace!("Received web request: {:?} {}", method, path);

        let future = with_timeout(
            Duration::from_secs(15),
            async {
                match (method, path.as_str()) {
                    (Method::Get, "/") => self.serve_static_content(
                        conn,
                        "text/html; charset=utf-8",
                        HTTP_INDEX_HTML
                    ).await,
                    (Method::Get, "/settings") => self.serve_static_content(
                        conn,
                        "text/html; charset=utf-8",
                        HTTP_SETTINGS_HTML
                    ).await,
                    (Method::Post, "/settings") => self.handle_post_settings(conn).await,
                    (Method::Get, "/success") => self.serve_static_content(
                        conn,
                        "text/html; charset=utf-8",
                        HTTP_SETTINGS_SAVE_SUCCESS_HTML
                    ).await,
                    (Method::Get, "/api/settings") => self.handle_api_get_settings(conn).await,
                    (Method::Put, "/api/settings") => self.handle_put_settings(conn).await,
                    (Method::Get, "/api/model3d") => self.handle_get_model3d(conn).await,
                    (Method::Put, "/api/model3d") => self.handle_put_model3d(conn).await,
                    (Method::Get, "/swagger") => self.serve_static_content(
                        conn,
                        "text/html; charset=utf-8",
                        HTTP_SWAGGER_HTML
                    ).await,
                    (Method::Get, "/swagger/openapi.json") => self.serve_static_content(
                        conn,
                        "application/json; charset=utf-8",
                        HTTP_OPENAPI_JSON
                    ).await,
                    (Method::Get, "/assets/base_style.css") => self.serve_static_content(
                        conn,
                        "text/css",
                        HTTP_BASE_STYLE_CSS
                    ).await,
                    (Method::Get, "/assets/cog_icon.svg") => self.serve_static_content(
                        conn,
                        "image/svg+xml",
                        HTTP_COG_ICON
                    ).await,
                    (Method::Get, "/favicon.ico") => self.serve_static_content(
                        conn,
                        "image/x-icon",
                        HTTP_FAV_ICON
                    ).await,
                    (Method::Get, "/api/datafull") => self.handle_get_api_data_full(conn).await,
                    (Method::Get, "/api/data") => self.handle_get_api_data(conn).await,
                    (Method::Get, "/api/storage") => self.handle_get_api_storage_info(conn).await,
                    (Method::Get, "/api/file-system-stats") => self.handle_get_api_storage_info(conn).await,
                    _ => self.handle_not_found(conn).await,
                }
            }
        );

        let method_string = match method {
            Method::Get => "GET",
            Method::Put => "PUT",
            Method::Post => "POST",
            Method::Delete => "DELETE",
            Method::Patch => "PATCH",
            _ => "other"
        };

        match future.await {
            Ok(value) => value,
            Err(_error) => self.setup_500_request_timeout(conn, method_string, &path).await
        }
    }
}

/// Get the stats for the file system
fn get_fs_stat(
    fs: MutexGuard<'_, NoopRawMutex, Filesystem<LittleFsStorage<'static>>>
) -> Result<FileSystemStat, Error> {
    let total_bytes = STORAGE_BLOCK_SIZE * STORAGE_BLOCK_COUNT;
    let used_bytes = match fs.fs_size() {
        Ok(used_blocks) => used_blocks * STORAGE_BLOCK_SIZE,
        Err(error) => return Err(error)
    };

    return Ok(
        FileSystemStat {
            total_bytes: total_bytes,
            used_bytes: used_bytes,
            free_bytes: total_bytes - used_bytes
        }
    );
}

/// Stats from the file system
struct FileSystemStat {
    /// The total size of the filesytem
    pub total_bytes: u32,
    /// The used bytes of the filesystem
    pub used_bytes: u32,
    /// The free bytes on the filesystem
    pub free_bytes: u32
}

/// The settings from the application
struct Settings {
    main_mesh_name: String,
    lidar_camera_name: String,
    wifi_ssid: String,
    wifi_password: String,
}

/// Parses the settings from json
/// 
/// ## Parameters
/// - value: The json to parse
fn parse_settings_from_json<'text, 'raw>(
    value: nojson::RawJsonValue<'text, 'raw>,
) -> Result<Settings, nojson::JsonParseError> {
    let main_mesh_name: &str = value.to_member("main_mesh_name")?.required()?.try_into()?;
    let lidar_camera_name: &str = value.to_member("lidar_camera_name")?.required()?.try_into()?;
    let wifi_ssid: &str = value.to_member("wifi_ssid")?.required()?.try_into()?;
    let wifi_password: &str = value.to_member("wifi_password")?.required()?.try_into()?;

    Ok(Settings {
        main_mesh_name: main_mesh_name.to_string(),
        lidar_camera_name: lidar_camera_name.to_string(),
        wifi_ssid: wifi_ssid.to_string(),
        wifi_password: wifi_password.to_string(),
    })
}

/// Parses the settings from the www-form
/// 
/// ## Parameters
/// - value: The json to parse
fn parse_settings_from_www_form(
    form: Form,
) -> Result<Settings, WwwFormParseError> {
    let mut main_mesh_name: Option<String> = None;
    let mut lidar_camera_name: Option<String> = None;
    let mut wifi_ssid: Option<String> = None;
    let mut wifi_password: Option<String> = None;

    for (key, value) in form {
        let mut key_string = heapless::String::<64>::new();
        let mut value_string = heapless::String::<128>::new();

        for byte in key {
            key_string.push(char::from(byte)).unwrap();
        }

        for byte in value {
            value_string.push(char::from(byte)).unwrap();
        }

        match key_string.as_str() {
            "main_mesh_name" => main_mesh_name = Some(value_string.to_string()),
            "lidar_camera_name" => lidar_camera_name = Some(value_string.to_string()),
            "wifi_ssid" => wifi_ssid = Some(value_string.to_string()),
            "wifi_password" => wifi_password = Some(value_string.to_string()),
            _ => {}
        }
    }

    Ok(Settings {
        main_mesh_name: main_mesh_name.unwrap_or_default(),
        lidar_camera_name: lidar_camera_name.unwrap_or_default(),
        wifi_ssid: wifi_ssid.unwrap_or_default(),
        wifi_password: wifi_password.unwrap_or_default(),
    })
}

#[derive(Debug)]
enum JsonParseError {
    BodyTooLong,
    ReaderReadFailed,
    Utf8ConvertError { error: core::str::Utf8Error },
    JsonParseError { error: nojson::JsonParseError }
}

#[derive(Debug)]
enum WwwFormParseError {
    BodyTooLong,
    ReaderReadFailed,
    Utf8ConvertError { error: core::str::Utf8Error },
    JsonParseError { error: nojson::JsonParseError }
}

/// Parses the json from the async reader into the target object
/// 
/// ## Parameters
/// - reader: The async reader to read from
/// - callback: The Function for parsing the RawJsonValue to the targetobject
async fn parse_json_from_reader<const BUFFER_SIZE: usize, R, TResult, B>(
    reader: &mut R,
    callback: B,
) -> Result<TResult, JsonParseError>
where
    R: Read,
    B: for<'text, 'raw> FnOnce(
        nojson::RawJsonValue<'text, 'raw>,
    ) -> Result<TResult, nojson::JsonParseError>,
{
    debug!("parse_json_from_reader > Was called...");

    let mut buffer = [0u8; BUFFER_SIZE];
    let mut len = 0;

    trace!("parse_json_from_reader > Reading the reader async...");
    while len < buffer.len() {
        let read_bytes = reader.read(&mut buffer[len..]).await
            .map_err(|_| JsonParseError::ReaderReadFailed)?;

        if read_bytes == 0 {
            break;
        }
        len += read_bytes;
    }
    trace!("parse_json_from_reader > Reading done.");

    if len == BUFFER_SIZE {
        let mut extra_byte = [0u8; 1];
        if reader.read(&mut extra_byte).await
            .map_err(|_| JsonParseError::ReaderReadFailed)? > 0
        {
            return Err(JsonParseError::BodyTooLong);
        }
    }

    trace!("parse_json_from_reader > Creating the string from the bytes...");
    let json_string = match core::str::from_utf8(&buffer[..len]) {
        Err(error) => {
            error!("handle_put_settings > Error while converting the body bytes to utf8 string: {:?}", error);
            return Err(JsonParseError::Utf8ConvertError { error });
        }
        Ok(string) => string
    };
    trace!("parse_json_from_reader > String created.");

    trace!("parse_json_from_reader > Parsing the string into raw json...");
    let raw = match nojson::RawJson::parse(json_string) {
        Ok(value) => value,
        Err(error) => {
            error!("handle_put_settings > Error while parsing the json: {:?}", error);
            return Err(JsonParseError::JsonParseError { error });
        }
    };
    trace!("parse_json_from_reader > String was parsed to json.");

    trace!("parse_json_from_reader > Parsing the raw json to the traget object...");
    let parsed_object = match callback(raw.value()) {
        Ok(value) => value,
        Err(error) => {
            error!("handle_put_settings > Error while parsing the json: {:?}", error);
            return Err(JsonParseError::JsonParseError { error });
        }
    };
    trace!("parse_json_from_reader > Parsing to the target object done.");

    return Ok(parsed_object);
}

/// Parses the www-form-urlencoded from the async reader into the target object
/// 
/// ## Parameters
/// - reader: The async reader to read from
/// - callback: The Function for parsing the www-form-urlencoded to the targetobject
async fn parse_www__form_urlencoded_from_reader<const BUFFER_SIZE: usize, R, TResult, B>(
    reader: &mut R,
    callback: B,
) -> Result<TResult, WwwFormParseError>
where
    R: Read,
    B: FnOnce(
        Form,
    ) -> Result<TResult, WwwFormParseError>,
{
    debug!("parse_www__form_urlencoded_from_reader > Was called...");

    let mut buffer = [0u8; BUFFER_SIZE];
    let mut len = 0;

    trace!("parse_www__form_urlencoded_from_reader > Reading the reader async...");
    while len < buffer.len() {
        let read_bytes = reader.read(&mut buffer[len..]).await
            .map_err(|_| WwwFormParseError::ReaderReadFailed)?;

        if read_bytes == 0 {
            break;
        }
        len += read_bytes;
    }
    trace!("parse_www__form_urlencoded_from_reader > Reading done.");

    if len == BUFFER_SIZE {
        let mut extra_byte = [0u8; 1];
        if reader.read(&mut extra_byte).await
            .map_err(|_| WwwFormParseError::ReaderReadFailed)? > 0
        {
            return Err(WwwFormParseError::BodyTooLong);
        }
    }

    trace!("parse_www__form_urlencoded_from_reader > Creating the string from the bytes...");
    let body_string = match core::str::from_utf8(&buffer[..len]) {
        Err(error) => {
            error!("parse_www__form_urlencoded_from_reader > Error while converting the body bytes to utf8 string: {:?}", error);
            return Err(WwwFormParseError::Utf8ConvertError { error });
        }
        Ok(string) => string
    };
    trace!("parse_www__form_urlencoded_from_reader > String created.");

    trace!("parse_www__form_urlencoded_from_reader > Parsing the from in the body...");
    let parsed_object = match callback(Form::new(body_string)) {
        Ok(value) => value,
        Err(error) => return Err(error)
    };
    trace!("parse_www__form_urlencoded_from_reader > Body was parsed.");

    return Ok(parsed_object);
}

enum SaveSettingsError<T>
where
    T: Read + Write
{
    ContentTypeWrong {
        expected_type: heapless::String<32>,
        acutal_type: heapless::String<32>
    },
    ContentCapacityError { error: CapacityError },
    HttpError { error: edge_http::io::Error<T::Error> },
    BodyTooLong { max_length: usize },
    JsonParseError { error: JsonParseError },
    OpenFileError { error: littlefs_rust::Error },
    WriteFileError { error: littlefs_rust::Error }
}