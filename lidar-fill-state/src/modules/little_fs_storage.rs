use log::error ;
use esp_println as _;
use littlefs_rust::{ Storage };
use esp_storage::FlashStorage;

/// Small implementation of a file storage
pub struct LittleFsStorage<'d> {
    /// The flash storage the is used
    flash: FlashStorage<'d>,

    /// The index at which the storage partition starts
    storage_partition_start_index: u32,

    /// The size of a storage block
    storage_block_size: u32
}

impl<'d> LittleFsStorage<'d> {
    /// Initializes the struct
    /// 
    /// - **flash**: The flash storage the is used
    /// - **storage_block_size**: The size of a storage block
    /// - **storage_partition_start_index**: The index at which the storage partition starts
    pub fn new(flash: FlashStorage<'d>, storage_block_size: u32, storage_partition_start_index: u32) -> Self {
        Self { flash, storage_block_size, storage_partition_start_index }
    }

    fn address(&self, block: u32, offset: u32) -> u32 {
        self.storage_partition_start_index + block * self.storage_block_size + offset
    }
}

impl Storage for LittleFsStorage<'_> {
    fn read(
        &mut self,
        block: u32,
        offset: u32,
        buf: &mut [u8],
    ) -> Result<(), littlefs_rust::Error> {
        let address = self.address(block, offset);

        let result = self.flash
            .read_nor(address, buf)
            .map_err(|e| {
                error!(
                    "LFS READ FAILED: block={} offset={} len={} address=0x{:08X} error={:?}",
                    block,
                    offset,
                    buf.len(),
                    address,
                    e
                );

                littlefs_rust::Error::Io
            });

        result
    }


    fn write(
        &mut self,
        block: u32,
        offset: u32,
        data: &[u8],
    ) -> Result<(), littlefs_rust::Error> {
        let address = self.address(block, offset);

        self.flash
            .write_nor(address, data)
            .map_err(|_| {
                error!(
                    "LFS WRITE FAILED: block={} offset={} len={} address=0x{:08X}",
                    block,
                    offset,
                    data.len(),
                    address
                );

                littlefs_rust::Error::Io
            })?;

        Ok(())
    }

    fn erase(
        &mut self,
        block: u32,
    ) -> Result<(), littlefs_rust::Error> {
        let address = self.address(block, 0);

        self.flash
            .erase(address, address + self.storage_block_size)
            .map_err(|_| {
                error!(
                    "LFS ERASE FAILED: block={} address=0x{:08X}",
                    block,
                    address
                );

                littlefs_rust::Error::Io
            })?;

        Ok(())
    }


    fn sync(&mut self) -> Result<(), littlefs_rust::Error> {
        Ok(())
    }
}