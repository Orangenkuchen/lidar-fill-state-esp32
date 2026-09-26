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
    blocking_mutex::raw::NoopRawMutex,
    mutex::Mutex,
};

/// The html of the index page of the webserver
const HTTP_INDEX_HTML: &str = include_str!("../../../web/index.html");
/// The html of the upload page of the webserver
const HTTP_UPLOAD_HTML: &str = include_str!("../../../web/upload.html");
/// The base css of the web pages of the webserver
const HTTP_BASE_STYLE_CSS: &str = include_str!("../../../web/base_style.css");

use crate::modules::little_fs_storage::LittleFsStorage;

pub struct HttpHandler {
    pub filesystem: &'static Mutex<
        NoopRawMutex,
        Filesystem<LittleFsStorage<'static>>,
    >,
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
            _ => self.handle_not_found(conn).await,
        }
    }
}