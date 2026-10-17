use log::{debug, info, trace};
use embedded_io_async::{Read, Write};
use esp_println as _;
use littlefs_rust::{
    Filesystem, OpenFlags
};
use embassy_sync::{
    blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex}, mutex::Mutex,
};
use static_cell::StaticCell;
use crate::modules::{
    little_fs_storage::LittleFsStorage,
};

/// The Path of the 3D-Model in the filesystem
const GLB_FILE_PATH: &str = "Model.glb";
/// The Path of the Settings-File in the filesystem
const SETTINGS_JSON_FILE_PATH: &str = "Settings.json";

const BLOCK_SIZE: usize = 1024;

static FILE_BUFFER: Mutex<CriticalSectionRawMutex, [u8; BLOCK_SIZE]> =
    Mutex::new([0; BLOCK_SIZE]);

pub enum FileManagerError<E> {
    Fs(littlefs_rust::Error),
    Generic(E),
}

pub struct FileManager {
    pub filesystem: &'static Mutex<
        NoopRawMutex,
        Filesystem<LittleFsStorage<'static>>,
    >,
}

impl FileManager {
    /// Saves the bytes from the reader as the file for the 3d-model
    /// 
    /// ## Parameters
    /// - reader: The reader with the bytes of the file
    pub async fn save_3dmodel<R, const BLOCK_SIZE: usize>(
        &self,
        reader: &mut R,
    ) -> Result<(), FileManagerError<R::Error>>
    where
        R: Read,
    {
        info!("save_3dmodel > Was called...");

        debug!("save_3dmodel > Calling save_file...");
        let result = self.save_file::<_, BLOCK_SIZE>(GLB_FILE_PATH, reader).await;
        trace!("save_3dmodel > save_file is done.");

        return result;
    }

    /// Loads the bytes of the file for the 3d-model
    /// 
    /// ## Paramters
    /// - writer: The bytes of the file will be written to the writer
    pub async fn load_3dmodel<W, const BLOCK_SIZE: usize>(
        &self, 
        writer: &mut W,
    ) -> Result<(), FileManagerError<W::Error>>
    where
        W: Write,
    {
        info!("load_3dmodel > Was called...");

        debug!("save_3dmodel > Calling load_file...");
        let result = self.load_file::<_, BLOCK_SIZE>(
            GLB_FILE_PATH,
            writer,
        ).await;
        trace!("save_3dmodel > load_file is done.");

        return result;
    }

    /// Loads the bytes of the file for the settings
    /// 
    /// ## Paramters
    /// - writer: The bytes of the file will be written to the writer
    pub async fn load_settings<W, const BLOCK_SIZE: usize>(
        &self, 
        writer: &mut W,
    ) -> Result<(), FileManagerError<W::Error>>
    where
        W: Write,
    {
        info!("load_settings > Was called...");

        debug!("load_settings > Calling load_file...");
        let result = self.load_file::<_, BLOCK_SIZE>(
            SETTINGS_JSON_FILE_PATH,
            writer,
        ).await;
        trace!("load_settings > load_file is done.");

        return result;
    }

    /// Loads the size of the 3d-model file in bytes
    /// 
    /// ## Parameters
    /// - path: The path of the file
    pub async fn get_3dmodel_size<const BLOCK_SIZE: usize>(
        &self, 
    ) -> Result<u32, littlefs_rust::Error>
    {
        info!("get_3dmodel_size > Was called...");

        debug!("get_3dmodel_size > Calling get_file_size...");
        let result = self.get_file_size(GLB_FILE_PATH).await;
        trace!("get_3dmodel_size > get_file_size is done.");

        return result;
    }
    
    // pub async fn load_metadata_from_3dmodel<const BLOCK_SIZE: usize>(
    //     &self, 
    // ) -> Result<u32, littlefs_rust::Error>
    // {
    //     info!("load_file > Was called...");

    //     debug!("load_file > Waiting for a lock on the file system...");
    //     let fs = self.filesystem.lock().await;
    //     trace!("load_file > Filesystem lock was aquired.");

    //     debug!("load_file > Trying to open the file \"{}\" ...", GLB_FILE_PATH);
    //     let file = match fs.open(GLB_FILE_PATH, OpenFlags::READ) {
    //         Ok(file) => file,
    //         Err(error) => return Err(FileManagerError::Fs(error))
    //     };
    //     trace!("load_file > File was successfully opend.");

    //     parse_file_data(file, )

    //     let mut buffer = [0u8; BLOCK_SIZE];

    //     debug!("load_file > Starting to send the file via the http_response...");
    //     loop {
    //         let n = match file.read(&mut buffer) {
    //             Ok(result) => result,
    //             Err(error) => return Err(FileManagerError::Fs(error))
    //         };

    //         if n == 0 {
    //             break;
    //         }

    //         match writer.write_all(&buffer[..n as usize]).await {
    //             Ok(()) => {}
    //             Err(error) => return Err(FileManagerError::Generic(error))
    //         };
    //     }
    //     trace!("load_file > Sending of the file is done. Request completed.");

    //     return Ok(1);
    // }

    /// Saves the bytes from the reader in the fs
    /// 
    /// ## Paramters
    /// - path: The path of the file to save to
    /// - reader: The reader that delivers the bytes to save
    pub async fn save_file<R, const BLOCK_SIZE: usize>(
        &self,
        path: &str,
        reader: &mut R,
    ) -> Result<(), FileManagerError<R::Error>>
    where
        R: Read,
    {
        info!("save_file > Was called...");

        debug!("save_file > Waiting for a lock on the file system...");
        let fs = self.filesystem.lock().await;
        trace!("save_file > Filesystem lock was aquired.");

        debug!("save_file > Removing the existing file, if present...");
        match fs.remove(path) {
            Ok(()) | Err(littlefs_rust::Error::NoEntry) => {},
            Err(error) => return Err(FileManagerError::Fs(error))
        }

        debug!("save_file > Trying to open the file \"{}\" ...", path);
        let file: littlefs_rust::File<'_, LittleFsStorage<'static>> = match fs.open(path, OpenFlags::WRITE | OpenFlags::CREATE) {
            Ok(file) => file,
            Err(error) => return Err(FileManagerError::Fs(error))
        };
        trace!("save_file > File was successfully opend.");

        let mut buffer = FILE_BUFFER.lock().await;

        debug!("save_file > Staring to write the model to the filesystem...");
        loop {
            let n = match reader.read(&mut *buffer).await {
                Ok(result) => result,
                Err(error) => return Err(FileManagerError::Generic(error))
            };

            if n == 0 {
                break;
            }

            match file.write(&buffer[..n]) {
                Ok(_) => {},
                Err(error) => return Err(FileManagerError::Fs(error))
            };
        }
        trace!("save_file > Writing of the file is done.");

        match file.close() {
            Ok(()) => {},
            Err(error) => return Err(FileManagerError::Fs(error))
        }

        return Ok(());
    }

    /// Loads the file and writes the bytes to the writer
    /// 
    /// ## Parameters
    /// - path: The path of the file
    /// - writer: The bytes will be written with this writer
    pub async fn load_file<W, const BLOCK_SIZE: usize>(
        &self,
        path: &str,
        writer: &mut W,
    ) -> Result<(), FileManagerError<W::Error>>
    where
        W: Write,
    {
        info!("load_file > Was called...");

        debug!("load_file > Waiting for a lock on the file system...");
        let fs = self.filesystem.lock().await;
        trace!("load_file > Filesystem lock was aquired.");

        debug!("load_file > Trying to open the file \"{}\" ...", path);
        let file = match fs.open(path, OpenFlags::READ) {
            Ok(file) => file,
            Err(error) => return Err(FileManagerError::Fs(error))
        };
        trace!("load_file > File was successfully opend.");

        let mut buffer = FILE_BUFFER.lock().await;

        debug!("load_file > Starting to send the file via the http_response...");
        loop {
            let n = match file.read(&mut *buffer) {
                Ok(result) => result,
                Err(error) => return Err(FileManagerError::Fs(error))
            };

            if n == 0 {
                break;
            }

            match writer.write_all(&buffer[..n as usize]).await {
                Ok(()) => {}
                Err(error) => return Err(FileManagerError::Generic(error))
            };
        }
        trace!("load_file > Sending of the file is done. Request completed.");

        return Ok(());
    }

    /// Loads the size of the file in bytes
    /// 
    /// ## Parameters
    /// - path: The path of the file
    pub async fn get_file_size(
        &self,
        path: &str,
    ) -> Result<u32, littlefs_rust::Error>
    {
        info!("get_file_size > Was called...");

        debug!("get_file_size > Waiting for a lock on the file system...");
        let fs = self.filesystem.lock().await;
        trace!("get_file_size > Filesystem lock was aquired.");

        debug!("get_file_size > Trying to open the file \"{}\" ...", path);
        let file = fs.open(path, OpenFlags::READ)?;
        trace!("get_file_size > File was successfully opend.");

        return Ok(file.size());
    }
}