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
use log::{error, info, trace};
use embedded_io_async::{Read, Write};
use esp_println as _;
use littlefs_rust::{
    Filesystem, OpenFlags
};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    blocking_mutex::raw::NoopRawMutex,
    mutex::Mutex,
};

/// The html of the index page of the webserver
const HTTP_INDEX_HTML: &str = include_str!("../../../web/index.html");
/// The html of the upload page of the webserver
const HTTP_UPLOAD_HTML: &str = include_str!("../../../web/upload.html");
/// The base css of the web pages of the webserver
const HTTP_BASE_STYLE_CSS: &str = include_str!("../../../web/assets/base_style.css");
/// The Fav-Icon of the website
const HTTP_FAV_ICON: &'static [u8] = include_bytes!("../../../web/favicon.ico");
/// The Path of the 3D-Model in the filesystem
const GLB_FILE_PATH: &str = "Model.glb";

use crate::modules::{
    little_fs_storage::LittleFsStorage,
    vl53l8cx_lidar_service::LidarReadingWatch,
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
    async fn handle_get_root<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "text/html; charset=utf-8")],
        ).await?;

        conn.write_all(HTTP_INDEX_HTML.as_bytes()).await?;
        Ok(())
    }

    async fn handle_get_hello<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "text/plain")],
        ).await?;

        conn.write_all(b"Hello from the ESP32-C6!\n").await?;
        Ok(())
    }

    async fn handle_get_upload<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "text/html; charset=utf-8")],
        ).await?;

        conn.write_all(HTTP_UPLOAD_HTML.as_bytes()).await?;
        Ok(())
    }

    async fn handle_post_upload<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        info!("Receiving file upload...");

        let mut buffer = [0u8; 8192];
        let mut total_bytes = 0usize;

        let fs = self.filesystem.lock().await;
        let file = match fs.open(
            "Test.bin",
            OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNC,
        ) {
            Ok(file) => file,
            Err(error) => {
                error!("Could not open Test.bin: {:?}", error);
                conn.initiate_response(
                    500,
                    Some("Internal Server Error"),
                    &[("Content-Type", "text/plain")],
                ).await?;
                conn.write_all(b"Upload failed\n").await?;
                return Ok(());
            }
        };

        loop {
            let n = conn.read(&mut buffer).await?;

            if n == 0 {
                break;
            }

            if let Err(error) = file.write(&buffer[..n]) {
                error!("File write failed: {:?}", error);
                conn.initiate_response(
                    500,
                    Some("Internal Server Error"),
                    &[("Content-Type", "text/plain")],
                ).await?;
                conn.write_all(b"Upload failed\n").await?;
                return Ok(());
            }

            total_bytes += n;
        }

        info!("File upload complete: {} bytes", total_bytes);

        conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "text/plain")],
        ).await?;

        conn.write_all(b"Upload successful\n").await?;
        Ok(())
    }

    async fn handle_get_file<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        info!("Sending Test.bin...");

        let mut buffer = [0u8; 1024];

        let fs = self.filesystem.lock().await;

        let file = match fs.open("Test.bin", OpenFlags::READ) {
            Ok(file) => file,
            Err(error) => {
                error!("Could not open Test.bin: {:?}", error);
                conn.initiate_response(
                    404,
                    Some("Not Found"),
                    &[("Content-Type", "text/plain")],
                ).await?;
                conn.write_all(b"File not found\n").await?;
                return Ok(());
            }
        };

        let mut content_length = heapless::String::<10>::new();
        write!(content_length, "{}", file.size()).unwrap();

        conn.initiate_response(
            200,
            Some("OK"),
            &[
                ("Content-Type", "application/octet-stream"),
                ("Content-Length", content_length.as_str()),
                ("Content-Disposition", "attachment; filename=\"Test.bin\"")
            ],
        ).await?;

        loop {
            let n = match file.read(&mut buffer) {
                Ok(n) => n,
                Err(error) => {
                    error!("File read failed: {:?}", error);
                    return Ok(());
                }
            };

            if n == 0 {
                return Ok(());
            }

            conn.write_all(&buffer[..n as usize]).await?;
        }
    }

    /// Returns the base_style.css
    async fn handle_get_base_css<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "text/css")],
        ).await?;

        conn.write_all(HTTP_BASE_STYLE_CSS.as_bytes()).await?;
        Ok(())
    }

    /// Returns the favicon.ico
    async fn handle_get_favicon<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "image/x-icon")],
        ).await?;

        conn.write_all(HTTP_FAV_ICON).await?;
        Ok(())
    }

    /// Returns the latest LIDAR data as JSON
    async fn handle_get_api_data_full<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "application/json")],
        ).await?;

        let mut receiver = self.reading_watch.receiver().unwrap();
        let lidar_reading = receiver.get().await;

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

        let mut response = self.json_response_buffer.lock().await;
        response.clear();
        write!(response, "{}", json).unwrap();
        conn.write_all(response.as_bytes()).await?;
        Ok(())
    }

    /// Returns the latest LIDAR data as JSON
    async fn handle_get_api_data<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        conn.initiate_response(
            200,
            Some("OK"),
            &[("Content-Type", "application/json")],
        ).await?;

        let mut receiver = self.reading_watch.receiver().unwrap();
        let lidar_reading = receiver.get().await;

        let mut distances: [i16; 64] = [0; 64];

        for i in 0..64 {
            distances[i] = lidar_reading.zones[i].distance_mm;
        }

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

        let mut response = self.json_response_buffer.lock().await;
        response.clear();
        write!(response, "{}", json).unwrap();
        conn.write_all(response.as_bytes()).await?;
        Ok(())
    }

    async fn handle_not_found<T, const N: usize>(
        &self,
        conn: &mut Connection<'_, T, N>,
    ) -> Result<(), edge_http::io::Error<T::Error>>
    where
        T: Read + Write,
    {
        conn.initiate_response(
            404,
            Some("Not Found"),
            &[("Content-Type", "text/plain")],
        ).await?;

        conn.write_all(b"404 Not Found\n").await?;
        Ok(())
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
            (Method::Get, "/") => self.handle_get_root(conn).await,
            (Method::Get, "/hello") => self.handle_get_hello(conn).await,
            (Method::Get, "/upload") => self.handle_get_upload(conn).await,
            (Method::Post, "/upload") => self.handle_post_upload(conn).await,
            (Method::Get, "/file") => self.handle_get_file(conn).await,
            (Method::Get, "/assets/base_style.css") => self.handle_get_base_css(conn).await,
            (Method::Get, "/favicon.ico") => self.handle_get_favicon(conn).await,
            (Method::Get, "/api/datafull") => self.handle_get_api_data_full(conn).await,
            (Method::Get, "/api/data") => self.handle_get_api_data(conn).await,
            _ => self.handle_not_found(conn).await,
        }
    }
}