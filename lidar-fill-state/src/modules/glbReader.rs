use alloc::{string::String, vec, vec::Vec};
use littlefs_rust::File;
use nojson::{JsonParseError, RawJson, RawJsonValue};
use crate::modules::little_fs_storage::LittleFsStorage;

/// The Size of the GLB. After the header the chunks start.
const GLB_Header_Size: u8 = 12;

/// The JSON-Type of a GLB-Chunk
const GLB_CHUNK_TYPE_JSON: u32 = 0x4E4F534A;
/// The BIN-Type of a GLB-Chunk
const GLB_CHUNK_TYPE_BIN: u32 = 0x004E4942;

/// The type of an chunk in the glb
#[repr(u32)]
pub enum GlbChunkType {
    /// The Chunk-Data has an unknown type
    Unknown = 0x0,
    /// The Chunk-Data is JSON
    Json = 0x4E4F534A,
    /// The Chunk-Data is Binary
    Binary = 0x004E4942
}

/// The errors that can happen while parsing the glb
pub enum GlbParseError {
    /// The File is not a glb according to the magic number
    FileIsNotGlb,
    /// The file is to short to be a valid glb
    FileTooShort,
    /// The file has an unknown version (supported: [v2])
    UnknownVersion { found_version: u32 },
    /// The file has a chunk which type is unknown
    UnknownChunkType { chunk_type: u32 },
    /// The Json-Data in the file was not found
    JsonDataNotFound,
    /// Reading data from the file failed
    FileReadFailed,
    /// The JSON chunk is not valid UTF-8 or JSON
    InvalidJson,
}

/// Checks the magic_number if it is a glb
/// 
/// ### Parameters:
/// 
/// **file_buffer**: Buffer of the start of the file
pub fn is_glb_file(file_buffer: &[u8]) -> bool {
    file_buffer.starts_with(b"glTF")
}


pub fn parse_file_data(file: File<'_, LittleFsStorage<'static>>) -> Result<GlbData, GlbParseError> {
    let header = match parse_glb_header(&file) {
        Ok(header) => header,
        Err(error) => return Err(error)
    };

    let mut read_file_bytes: u32 = header.read_bytes;

    if header.data.version != 2 {
        return Err(GlbParseError::UnknownVersion { found_version: header.data.version });
    }

    let first_chunk = match parse_glb_chunk(&file) {
        Ok(first_chunk) => first_chunk,
        Err(error) => return Err(error)
    };
    read_file_bytes += first_chunk.read_bytes;

    if first_chunk.data.chunk_type != GlbChunkType::Json as u32 {
        return Err(GlbParseError::JsonDataNotFound);
    }

    let json_length = first_chunk.data.length as usize;
    let mut json_bytes = vec![0; json_length];
    let bytes_read = file
        .read(&mut json_bytes)
        .map_err(|_| GlbParseError::FileReadFailed)?;

    if bytes_read != first_chunk.data.length {
        return Err(GlbParseError::FileTooShort);
    }

    let json_text = core::str::from_utf8(&json_bytes)
        .map_err(|_| GlbParseError::InvalidJson)?;
    let raw_json = RawJson::parse(json_text)
        .map_err(|_| GlbParseError::InvalidJson)?;
    let json = GlbJsonChunk::try_from(raw_json.value())
        .map_err(|_| GlbParseError::InvalidJson)?;

    // TODO: Pull information out of parsed json

    Ok(GlbData {
        header: header.data,
        json,
    })
}

/// Parses the header from the glb file
/// 
/// Must be at the start of the file where the header is.
/// 
/// ### Parameters
/// - **file**: The file to parse
fn parse_glb_header(file: &File<'_, LittleFsStorage<'static>>) -> Result<PraseResult<GlbHeader>, GlbParseError> {
    let mut header_bytes: [u8; 12] = [0;12];
    
    let bytes_read = file.read(&mut header_bytes).unwrap(); //TODO: No Unwrap

    if bytes_read < 12 {
        return Err(GlbParseError::FileTooShort);
    }
    if is_glb_file(&header_bytes[0..4]) == false {
        return Err(GlbParseError::FileIsNotGlb);
    }

    let version = u32::from_le_bytes(header_bytes[4..8].try_into().unwrap());

    let total_length = u32::from_le_bytes(header_bytes[8..12].try_into().unwrap());
    
    return Ok(
        PraseResult {
            read_bytes: bytes_read,
            data: GlbHeader {
                version: version,
                file_length: total_length
            }
        }
    )
}


/// Parses the header of a chunk in the glb file
/// 
/// ### Parameters
/// - **file**: The file to parse
fn parse_glb_chunk(file: &File<'_, LittleFsStorage<'static>>) -> Result<PraseResult<GlbChunk>, GlbParseError> {
    let mut chunk_header_bytes: [u8; 8] = [0;8];

    let bytes_read = file.read(&mut chunk_header_bytes).unwrap(); //TODO: No Unwrap

    if bytes_read < 8 {
        return Err(GlbParseError::FileTooShort);
    }

    let chunk_length = u32::from_le_bytes(chunk_header_bytes[0..4].try_into().unwrap());
    let chunk_type = u32::from_le_bytes(chunk_header_bytes[4..8].try_into().unwrap());

    return Ok(
        PraseResult {
            read_bytes: bytes_read,
            data: GlbChunk {
                length: chunk_length,
                chunk_type
            }
        }
    );
}

/// The result of a parse
pub struct PraseResult<TData> {
    /// The amount of bytes that were consumed
    pub read_bytes: u32,
    /// The data that was parsed
    pub data: TData
}

/// The Header of an GLB file
pub struct GlbHeader {
    /// The Version of the file
    pub version: u32,

    /// The total lengths of the file including header
    pub file_length: u32
}

/// A chunk of data inside the glb
pub struct GlbChunk {
    /// The length of the data in the chunk
    length: u32,
    /// The type of the chunk
    chunk_type: u32
}

pub struct GlbJsonChunk {
    pub generated_by: Option<String>,
    pub meshes: Vec<GlbJsonMesh>,
    pub cameras: Vec<GlbJsonCamera>,
    pub accessors: Vec<GlbJsonAccessor>,
    pub buffer_views: Vec<GlbJsonBufferView>,
    pub nodes: Vec<GlbJsonNode>,
}

pub struct GlbJsonNode {
    pub mesh: Option<u32>,
    pub camera: Option<u32>,
    pub name: Option<String>,
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
}

pub struct GlbJsonCamera {
    pub name: Option<String>,
    pub perspective: GlbJsonCameraPerspective,
}

pub struct GlbJsonCameraPerspective {
    pub aspect_ratio: Option<f32>,
    pub yfov: f32,
}

pub struct GlbJsonMesh {
    pub name: Option<String>,
    pub primitives: Vec<GlbJsonPrimitive>,
}

pub struct GlbJsonPrimitive {
    pub indices: Option<u32>,
}

pub struct GlbJsonAccessor {
    pub buffer_view: Option<u32>,
}

pub struct GlbJsonBufferView {
    pub byte_offset: u32,
    pub byte_length: u32,
}

fn optional_member<'text, 'raw, T>(
    value: RawJsonValue<'text, 'raw>,
    name: &str,
) -> Result<Option<T>, JsonParseError>
where
    T: TryFrom<RawJsonValue<'text, 'raw>, Error = JsonParseError>,
{
    value
        .to_member(name)?
        .optional()
        .map(T::try_from)
        .transpose()
}

fn array_member<'text, 'raw, T>(
    value: RawJsonValue<'text, 'raw>,
    name: &str,
) -> Result<Vec<T>, JsonParseError>
where
    T: TryFrom<RawJsonValue<'text, 'raw>, Error = JsonParseError>,
{
    optional_member(value, name).map(|items| items.unwrap_or_default())
}

impl<'text, 'raw> TryFrom<RawJsonValue<'text, 'raw>> for GlbJsonChunk {
    type Error = JsonParseError;

    fn try_from(value: RawJsonValue<'text, 'raw>) -> Result<Self, Self::Error> {
        let asset = value.to_member("asset")?.required()?;

        Ok(Self {
            generated_by: optional_member(asset, "generator")?,
            meshes: array_member(value, "meshes")?,
            cameras: array_member(value, "cameras")?,
            accessors: array_member(value, "accessors")?,
            buffer_views: array_member(value, "bufferViews")?,
            nodes: array_member(value, "nodes")?,
        })
    }
}

impl<'text, 'raw> TryFrom<RawJsonValue<'text, 'raw>> for GlbJsonNode {
    type Error = JsonParseError;

    fn try_from(value: RawJsonValue<'text, 'raw>) -> Result<Self, Self::Error> {
        Ok(Self {
            mesh: optional_member(value, "mesh")?,
            camera: optional_member(value, "camera")?,
            name: optional_member(value, "name")?,
            rotation: optional_member(value, "rotation")?
                .unwrap_or([0.0, 0.0, 0.0, 1.0]),
            translation: optional_member(value, "translation")?
                .unwrap_or([0.0, 0.0, 0.0]),
        })
    }
}

impl<'text, 'raw> TryFrom<RawJsonValue<'text, 'raw>> for GlbJsonCamera {
    type Error = JsonParseError;

    fn try_from(value: RawJsonValue<'text, 'raw>) -> Result<Self, Self::Error> {
        Ok(Self {
            name: optional_member(value, "name")?,
            perspective: value.to_member("perspective")?.required()?.try_into()?,
        })
    }
}

impl<'text, 'raw> TryFrom<RawJsonValue<'text, 'raw>> for GlbJsonCameraPerspective {
    type Error = JsonParseError;

    fn try_from(value: RawJsonValue<'text, 'raw>) -> Result<Self, Self::Error> {
        Ok(Self {
            aspect_ratio: optional_member(value, "aspectRatio")?,
            yfov: value.to_member("yfov")?.required()?.try_into()?,
        })
    }
}

impl<'text, 'raw> TryFrom<RawJsonValue<'text, 'raw>> for GlbJsonMesh {
    type Error = JsonParseError;

    fn try_from(value: RawJsonValue<'text, 'raw>) -> Result<Self, Self::Error> {
        Ok(Self {
            name: optional_member(value, "name")?,
            primitives: value.to_member("primitives")?.required()?.try_into()?,
        })
    }
}

impl<'text, 'raw> TryFrom<RawJsonValue<'text, 'raw>> for GlbJsonPrimitive {
    type Error = JsonParseError;

    fn try_from(value: RawJsonValue<'text, 'raw>) -> Result<Self, Self::Error> {
        Ok(Self {
            indices: optional_member(value, "indices")?,
        })
    }
}

impl<'text, 'raw> TryFrom<RawJsonValue<'text, 'raw>> for GlbJsonAccessor {
    type Error = JsonParseError;

    fn try_from(value: RawJsonValue<'text, 'raw>) -> Result<Self, Self::Error> {
        Ok(Self {
            buffer_view: optional_member(value, "bufferView")?,
        })
    }
}

impl<'text, 'raw> TryFrom<RawJsonValue<'text, 'raw>> for GlbJsonBufferView {
    type Error = JsonParseError;

    fn try_from(value: RawJsonValue<'text, 'raw>) -> Result<Self, Self::Error> {
        Ok(Self {
            byte_offset: optional_member(value, "byteOffset")?.unwrap_or(0),
            byte_length: value.to_member("byteLength")?.required()?.try_into()?,
        })
    }
}

/// The Data of the Glb file
pub struct GlbData {
    /// The Header of the GLB file
    pub header: GlbHeader,
    /// Parsed data from the JSON chunk
    pub json: GlbJsonChunk,
}