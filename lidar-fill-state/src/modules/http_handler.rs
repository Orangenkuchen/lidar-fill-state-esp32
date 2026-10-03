use core::fmt::{Debug, Display, Write as FmtWrite};
use edge_http::{
    io::{
        server::{
            Connection,
            Handler,
        }
    },
    Method,
};
use log::{debug, error, info, trace};
use embedded_io_async::{Read, Write};
use esp_println as _;
use littlefs_rust::{
    Error, Filesystem, OpenFlags
};
use embassy_sync::{
    blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex}, mutex::{Mutex, MutexGuard},
};

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
const HTTP_INDEX_HTML: &'static [u8] = include_bytes!("../../../web/index.html");
/// The html of the settings page of the webserver
const HTTP_SETTINGS_HTML: &'static [u8] = include_bytes!("../../../web/settings.html");
/// The base css of the web pages of the webserver
const HTTP_BASE_STYLE_CSS: &'static [u8] = include_bytes!("../../../web/assets/base_style.css");
/// The Fav-Icon of the website
const HTTP_FAV_ICON: &'static [u8] = include_bytes!("../../../web/favicon.ico");
/// The Path of the 3D-Model in the filesystem
const GLB_FILE_PATH: &str = "Model.glb";
/// The size of a storage block
const STORAGE_BLOCK_SIZE: u32 = 4 * 1_024; // TODO: Mit main.rs const verbinden
/// The amount of blocks in the storage
const STORAGE_BLOCK_COUNT: u32 = 528; // TODO: Mit main.rs const verbinden
/// The size of the http-connection buffer
const HTTP_CONNECTION_BUFFER_SIZE: usize = 8192;

use crate::modules::{
    little_fs_storage::LittleFsStorage, vl53l8cx_lidar_service::LidarReadingWatch,
};

pub type LidarJsonResponseBuffer =
    Mutex<CriticalSectionRawMutex, heapless::String<16_384>>;

pub struct HttpHandler {
    pub filesystem: &'static Mutex<
        NoopRawMutex,
        Filesystem<LittleFsStorage<'static>>,
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
        info!("handle_post_upload > Was called...");

        // TODO: Save settings

        conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "text/plain")],
        ).await?;

        conn.write_all(b"Upload successful\n").await?;
        Ok(())
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

        let mut buffer = [0u8; HTTP_CONNECTION_BUFFER_SIZE];

        debug!("handle_get_model3d > Waiting for a lock on the file system...");
        let fs = self.filesystem.lock().await;
        trace!("handle_get_model3d > Filesystem lock was aquired.");

        debug!("handle_get_model3d > Trying to open the 3D-Model file (\"{}\") ...", GLB_FILE_PATH);
        let file = match fs.open(GLB_FILE_PATH, OpenFlags::READ) {
            Ok(file) => file,
            Err(error) => {
                error!("Could not open {}: {:?}", GLB_FILE_PATH, error);
                conn.initiate_response(
                    404,
                    Some("Not Found"),
                    &[("Content-Type", "text/plain")],
                ).await?;
                conn.write_all(b"File not found\n").await?;
                return Ok(());
            }
        };
        trace!("handle_get_model3d > File was successfully opend.");

        let mut content_length = heapless::String::<10>::new();
        write!(content_length, "{}", file.size()).unwrap();
        let mut content_disposition = heapless::String::<64>::new();
        write!(content_disposition, "attachment; filename=\"{}\"", GLB_FILE_PATH).unwrap();

        debug!("handle_get_model3d > Initialising the response...");
        conn.initiate_response(
            200,
            Some("OK"),
            &[
                ("Content-Type", "model/gltf-binary"),
                ("Content-Length", content_length.as_str()),
                ("Content-Disposition", content_disposition.as_str())
            ],
        ).await?;
        trace!("handle_get_model3d > Response was initialized.");

        debug!("handle_get_model3d > Starting to send the file via the http_response...");
        loop {
            let n = match file.read(&mut buffer) {
                Ok(n) => n,
                Err(error) => {
                    error!("handle_get_model3d > File read failed: {:?}", error);
                    return Ok(());
                }
            };

            if n == 0 {
                break;
            }

            conn.write_all(&buffer[..n as usize]).await?;
        }
        trace!("handle_get_model3d > Sending of the file is done. Request completed.");

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

        debug!("handle_put_model3d > Calling save_file_from_request_to_fs...");
        match self.save_file_from_request_to_fs(conn, GLB_FILE_PATH).await {
            Ok(()) => {},
            Err(error) => {
                match error {
                    WriteFileFromRequestError::Fs(error) => {
                        error!("Could not open file {} for writing: {:?}", GLB_FILE_PATH, error);
                    },
                    WriteFileFromRequestError::Http(error) => {
                        error!("Could not read from the request: {:?}", error);
                    }
                }

                conn.initiate_response(
                    500,
                    Some("Internal Server Error"),
                    &[("Content-Type", "text/plain")],
                ).await?;

                conn.write_all(b"Upload failed\n").await?;
                return Ok(());
            }
        };
        trace!("handle_put_model3d > Completed save_file_from_request_to_fs.");

        debug!("handle_put_model3d > Initialising the response...");
        conn.initiate_response(
            204,
            Some("NOCONTENT"),
            &[],
        ).await?;
        trace!("handle_put_model3d > Response was initialized. Request completed.");

        return Ok(())
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
        conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "application/json")],
        ).await?;
        trace!("handle_get_api_data_full > Response was initialized.");

        debug!("handle_get_api_data_full > Writing the json to the response...");
        let mut response = self.json_response_buffer.lock().await;
        response.clear();
        write!(response, "{}", json).unwrap();
        conn.write_all(response.as_bytes()).await?;
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
        conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "application/json")],
        ).await?;
        trace!("handle_get_api_data > Response was initialized.");

        debug!("handle_not_found > Writing the json to the response...");
        let mut response = self.json_response_buffer.lock().await;
        response.clear();
        write!(response, "{}", json).unwrap();
        conn.write_all(response.as_bytes()).await?;
        trace!("handle_not_found > Text was written. Request completed.");

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
                self.setup_500_code_on_connection(conn, None).await?;
                return Ok(());
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
        conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "application/json")],
        ).await?;
        trace!("handle_get_api_storage_info > Response was initialized.");

        let mut response = self.json_response_buffer.lock().await;
        response.clear();
        write!(response, "{}", json).unwrap();

        debug!("handle_get_api_storage_info > Writing the json to the response...");
        conn.write_all(response.as_bytes()).await?;
        trace!("handle_get_api_storage_info > Json was written. Request completed.");

        return Ok(())
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
        conn.initiate_response(
            404,
            Some("Not Found"),
            &[("Content-Type", "text/plain")],
        ).await?;
        trace!("handle_not_found > Response was initialized.");

        debug!("handle_not_found > Writing the text to the response...");
        conn.write_all(b"404 Not Found\n").await?;
        trace!("handle_not_found > Text was written. Request completed.");

        return Ok(())
    }

    /// Saves the file from the request-body to the filesystem
    /// 
    /// ## Parameters
    /// - conn: The request which body should be written
    /// - file_path: The path at which the file should be saved in the fs
    async fn save_file_from_request_to_fs<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
        file_path: &str,
    ) -> Result<(), WriteFileFromRequestError<T::Error>>
    where
        T: Read + Write,
    {
        info!("save_file_from_request_to_fs > Was called...");

        let mut buffer = [0u8; HTTP_CONNECTION_BUFFER_SIZE];
        let mut total_bytes = 0usize;

        debug!("save_file_from_request_to_fs > Waiting for a lock on the file system...");
        let fs = self.filesystem.lock().await;
        trace!("save_file_from_request_to_fs > Filesystem lock was aquired.");

        debug!("save_file_from_request_to_fs > Trying to open the file \"{}\" ...", file_path);
        let file = fs.open(
            file_path,
            OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNC,
        )?;
        trace!("save_file_from_request_to_fs > File was successfully opend.");

        debug!("save_file_from_request_to_fs > Staring to write the model to the filesystem...");
        loop {
            let n = conn.read(&mut buffer).await?;

            if n == 0 {
                break;
            }

            file.write(&buffer[..n])?;

            total_bytes += n;
        }
        trace!("save_file_from_request_to_fs > Writing of the file is done (written bytes {}).", total_bytes);

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

        debug!("serve_static_content > Initialising the response (200; {})...", mime_type);
        conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", mime_type)],
        ).await?;
        trace!("serve_static_content > Response was initialized.");

        debug!("serve_static_content > Writing the bytes ({}) to the response...", content_bytes.len());
        conn.write_all(content_bytes).await?;
        trace!("serve_static_content > Bytes was written. Request completed.");

        return Ok(());
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
        conn.initiate_response(
            500,
            Some("Internal Server Error"),
            &[("Content-Type", "text/plain")],
        ).await?;

        match body_text {
            None => {},
            Some(body_text) => {
                conn.write_all(body_text.as_bytes()).await?;
            }
        }

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
        let headers = conn.headers()?;

        trace!("Received web request: {:?} {}", headers.method, headers.path);

        match (headers.method, headers.path) {
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
            (Method::Get, "/model3d") => self.handle_get_model3d(conn).await,
            (Method::Put, "/model3d") => self.handle_put_model3d(conn).await,
            (Method::Get, "/assets/base_style.css") => self.serve_static_content(
                conn,
                "text/css",
                HTTP_BASE_STYLE_CSS
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