use core::{
    convert::{TryFrom, TryInto},
    option::Option,
    result::{Result, Result::Err, Result::Ok},
};

use alloc::{
    string::String,
    vec,
    vec::Vec,
};

use nojson::{JsonParseError, RawJson, RawJsonValue};

pub trait GlbReader {
    fn read(&mut self, buffer: &mut [u8]) -> Result<u32, ()>;
    fn seek(&mut self, position: u32) -> Result<(), ()>;
}

/// The Size of the GLB. After the header the chunks start.
const GLB_HEADER_SIZE: usize = 12;

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
#[derive(Debug)]
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
    /// The mesh that represents the main container was not found
    ContainerMeshNotFound,
    /// The camera that represents the lidar was not found
    LidarCameraNotFound
}

#[derive(Debug)]
pub enum ParseGlbBinPartError {
    LengthInvalid
}

/// Checks the magic_number if it is a glb
/// 
/// ### Parameters:
/// 
/// **file_buffer**: Buffer of the start of the file
pub fn is_glb_file(file_buffer: &[u8]) -> bool {
    file_buffer.starts_with(b"glTF")
}

pub fn parse_file_data<R: GlbReader>(
    file: &mut R,
    container_mesh_name: String,
    lidar_camera_name: String
) -> Result<GlbData, GlbParseError> {
    let header = match parse_glb_header(file) {
        Ok(header) => header,
        Err(error) => return Err(error)
    };

    let mut read_file_bytes: u32 = header.read_bytes;

    if header.data.version != 2 {
        return Err(GlbParseError::UnknownVersion { found_version: header.data.version });
    }

    let first_chunk = match parse_glb_chunk(file) {
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
    read_file_bytes += bytes_read;

    if bytes_read != first_chunk.data.length {
        return Err(GlbParseError::FileTooShort);
    }

    let json_text = core::str::from_utf8(&json_bytes)
        .map_err(|_| GlbParseError::InvalidJson)?;
    let raw_json = RawJson::parse(json_text)
        .map_err(|_| GlbParseError::InvalidJson)?;
    let json = GlbJsonChunk::try_from(raw_json.value())
        .map_err(|_| GlbParseError::InvalidJson)?;

    let mut container_mesh: Option<GlbMesh> = None;
    let mut lidar_camera: Option<GlbCamera> = None;
    
    for node in json.nodes {
        if node.mesh.is_some() &&
           node.name.as_deref().is_some_and(|f|f == container_mesh_name) {
            container_mesh = Some(
                GlbMesh {
                    vertecies: GlbChunkData { byte_offset: 0, byte_length: 0, accessor_type: AccessorType::SCALAR, component_type: ComponentType::I8 },
                    indicies: None,
                    normals: None,
                    rotation: node.rotation,
                    translation: node.translation,
                    scale: node.scale,
                    matrix: node.matrix,
                }
            )
        } else if node.camera.is_some() && 
                  node.name.is_some_and(|f|f == lidar_camera_name) {
            lidar_camera = Some(
                GlbCamera {
                    aspect_ratio: None,
                    fov_radian: None,
                    rotation: node.rotation,
                    translation: node.translation,
                    scale: node.scale,
                    matrix: node.matrix,
                }
            )
        }
    }

    let mut container_mesh = container_mesh
        .ok_or(GlbParseError::ContainerMeshNotFound)?;
    let mut lidar_camera = lidar_camera
        .ok_or(GlbParseError::LidarCameraNotFound)?;

    for camera in json.cameras {
        if camera.name.is_some_and(|f| f == lidar_camera_name) {
            lidar_camera.aspect_ratio = camera.perspective.aspect_ratio;
            lidar_camera.fov_radian = Some(camera.perspective.yfov)
        }
    }

    let mut container_json_mesh: Option<GlbJsonMesh> = None;
    for mesh in json.meshes {
        if mesh.name.as_deref() == Some(container_mesh_name.as_str()) {
            container_json_mesh = Some(mesh);
            break;
        }
    }

    let container_json_mesh = container_json_mesh
        .ok_or(GlbParseError::ContainerMeshNotFound)?;

    if container_json_mesh.primitives.len() < 1 || json.accessors.len() != json.buffer_views.len() {
        return Err(GlbParseError::InvalidJson); // TODO: Besserer Feler
    }

    // Read BIN chunk header
    let bin_chunk = parse_glb_chunk(file)
        .map_err(|_| GlbParseError::FileReadFailed)?;
    read_file_bytes += bin_chunk.read_bytes;

    // The reader is now positioned at the beginning of BIN data.
    let glb_bin_chunk_data_start = read_file_bytes;

    let mut buffer_views_absolute: Vec<GlbBufferViewPosition> = Vec::new();

    for i in 0..json.accessors.len() {
        let accessor = json.accessors.get(i).unwrap();

        buffer_views_absolute.push(
            GlbBufferViewPosition {
                length: json.buffer_views[i].byte_length,
                start: glb_bin_chunk_data_start + json.buffer_views[i].byte_offset,
                accessory_type: accessor.r#type,
                component_type: accessor.component_type
            }
        );
    }

    match container_json_mesh.primitives[0].attributes {
        None => {},
        Some(ref attributes ) => {
            let buffer_view_position: &GlbBufferViewPosition = match buffer_views_absolute.get(attributes.position as usize) {
                None => return Err(GlbParseError::InvalidJson),
                Some(element) => element
            };

            container_mesh.vertecies.byte_offset = buffer_view_position.start;
            container_mesh.vertecies.byte_length = buffer_view_position.length;
            container_mesh.vertecies.accessor_type = buffer_view_position.accessory_type;
            container_mesh.vertecies.component_type = buffer_view_position.component_type;

            match attributes.normal {
                None => {},
                Some(normal) => {
                    let buffer_view_position: &GlbBufferViewPosition = match buffer_views_absolute.get(normal as usize) {
                        None => return Err(GlbParseError::InvalidJson),
                        Some(element) => element
                    };

                    container_mesh.normals = Some(
                        GlbChunkData {
                            byte_offset: buffer_view_position.start,
                            byte_length: buffer_view_position.length,
                            accessor_type: buffer_view_position.accessory_type,
                            component_type: buffer_view_position.component_type
                        }
                    );
                }
            }
        }
    }

    match container_json_mesh.primitives[0].indices {
        None => {},
        Some(indicies) => {
            let buffer_view_position: &GlbBufferViewPosition = match buffer_views_absolute.get(indicies as usize) {
                None => return Err(GlbParseError::InvalidJson),
                Some(element) => element
            };

            container_mesh.indicies = Some(
                GlbChunkData {
                    byte_offset: buffer_view_position.start,
                    byte_length: buffer_view_position.length,
                    accessor_type: buffer_view_position.accessory_type,
                    component_type: buffer_view_position.component_type
                }
            );
        }
    }

    Ok(GlbData {
        header: header.data,
        container_mesh,
        lidar_camera,
    })
}

pub fn get_vec3_f32_from_reader<R: GlbReader>(
    reader: &mut R,
    offset: u32,
    length: u32
) -> Result<Vec<[f32; 3]>, ParseGlbBinPartError> {
    let mut bytes = vec![0u8; length as usize];

    reader.seek(offset).unwrap();
    reader.read(&mut bytes).unwrap();

    if length % 12 as u32 != 0 {
        return Err(ParseGlbBinPartError::LengthInvalid);
    }

    let mut vertices = Vec::new();

    for chunk in bytes.chunks_exact(12) {
        let x = f32::from_le_bytes(chunk[0..4].try_into().unwrap());
        let y = f32::from_le_bytes(chunk[4..8].try_into().unwrap());
        let z = f32::from_le_bytes(chunk[8..12].try_into().unwrap());

        vertices.push([x, y, z]);
    }

    Ok(vertices)
}

pub fn get_scalar_u16_from_reader<R: GlbReader>(
    reader: &mut R,
    offset: u32,
    length: u32
) -> Result<Vec<u16>, ParseGlbBinPartError> {
    let mut bytes = vec![0u8; length as usize];

    reader.seek(offset).unwrap();
    reader.read(&mut bytes).unwrap();

    if length % 2 as u32 != 0 {
        return Err(ParseGlbBinPartError::LengthInvalid);
    }

    let mut vertices = Vec::new();

    for chunk in bytes.chunks_exact(2) {
        vertices.push(
            u16::from_le_bytes(chunk.try_into().unwrap())
        );
    }

    Ok(vertices)
}

/// Parses the header from the glb file
/// 
/// Must be at the start of the file where the header is.
/// 
/// ### Parameters
/// - **file**: The file to parse
fn parse_glb_header<R: GlbReader>(
    file: &mut R,
) -> Result<PraseResult<GlbHeader>, GlbParseError> {
    let mut header_bytes: [u8; GLB_HEADER_SIZE] = [0;GLB_HEADER_SIZE];
    
    let bytes_read = file.read(&mut header_bytes).unwrap(); //TODO: No Unwrap

    if bytes_read < GLB_HEADER_SIZE as u32 {
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
fn parse_glb_chunk<R: GlbReader>(file: &mut R) -> Result<PraseResult<GlbChunk>, GlbParseError> {
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
    pub rotation: Option<[f32; 4]>,
    pub translation: Option<[f32; 3]>,
    pub scale: Option<[f32; 3]>,
    pub matrix: Option<[f32; 16]>,
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
    pub attributes: Option<GlbJsonAttribute>
}

pub struct GlbJsonAttribute {
    pub position: u32,
    pub normal: Option<u32>
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ComponentType {
    I8 = 5120,
    U8 = 5121,
    I16 = 5122,
    U16 = 5123,
    U32 = 5125,
    F32 = 5126
}
impl TryFrom<u32> for ComponentType {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        return match value {
            5120 => Ok(Self::I8),
            5121 => Ok(Self::U8),
            5122 => Ok(Self::I16),
            5123 => Ok(Self::U16),
            5125 => Ok(Self::U32),
            5126 => Ok(Self::F32),
            _ => Err(())
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum AccessorType {
    /// Number of Components = 1
    SCALAR,

    /// Number of Components = 2
    VEC2,

    /// Number of Components = 3
    VEC3,

    /// Number of Components = 4
    VEC4,

    /// Number of Components = 4
    MAT2,

    /// Number of Components = 9
    MAT3,

    /// Number of Components = 16
    MAT4
}
impl TryFrom<&str> for AccessorType {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        return match value {
            "SCALAR" => Ok(Self::SCALAR),
            "VEC2" => Ok(Self::VEC2),
            "VEC3" => Ok(Self::VEC3),
            "VEC4" => Ok(Self::VEC4),
            "MAT2" => Ok(Self::MAT2),
            "MAT3" => Ok(Self::MAT3),
            "MAT4" => Ok(Self::MAT4),
            _ => Err(())
        }
    }
}

pub struct GlbJsonAccessor {
    pub buffer_view: u32,
    pub component_type: ComponentType,
    pub count: u32,
    pub r#type: AccessorType
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
            rotation: optional_member(value, "rotation")?,
            translation: optional_member(value, "translation")?,
            scale: optional_member(value, "scale")?,
            matrix: optional_member(value, "matrix")?
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
            attributes: optional_member(value,"attributes")?
        })
    }
}

impl<'text, 'raw> TryFrom<RawJsonValue<'text, 'raw>> for GlbJsonAttribute {
    type Error = JsonParseError;

    fn try_from(value: RawJsonValue<'text, 'raw>) -> Result<Self, Self::Error> {
        Ok(Self {
            normal: optional_member(value, "NORMAL")?,
            position: value.to_member( "POSITION")?.required()?.try_into()?,
        })
    }
}

impl<'text, 'raw> TryFrom<RawJsonValue<'text, 'raw>> for GlbJsonAccessor {
    type Error = JsonParseError;

    fn try_from(value: RawJsonValue<'text, 'raw>) -> Result<Self, Self::Error> {
        let type_string: &str = value.to_member("type")?.required()?.try_into().unwrap();
        let component_type_string: u32 = value.to_member("componentType")?.required()?.try_into().unwrap();
        
        Ok(Self {
            buffer_view: value.to_member("bufferView")?.required()?.try_into()?,
            count: value.to_member("count")?.required()?.try_into()?,
            r#type: type_string.try_into().unwrap(),
            component_type: component_type_string.try_into().unwrap()
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
    /// The mesh that is the container for the fill measurment
    pub container_mesh: GlbMesh,
    pub lidar_camera: GlbCamera
}

pub struct GlbChunkData {
    // The offset relative to the start of the glb file where this chunk starts
    pub byte_offset: u32,
    /// The length of the chunk
    pub byte_length: u32,
    /// The structure of the values in the chunk
    pub accessor_type: AccessorType,
    /// The type of the values in the structure in the chunk
    pub component_type: ComponentType
}

pub struct GlbMesh {
    /// The Data of the Vertecies of the Mesh
    pub vertecies: GlbChunkData,
    /// The Data of the Indecies of the Mesh
    pub indicies: Option<GlbChunkData>,
    /// The Data of the Normals of the Mesh
    pub normals: Option<GlbChunkData>,
    pub rotation: Option<[f32; 4]>,
    pub translation: Option<[f32; 3]>,
    pub scale: Option<[f32; 3]>,
    pub matrix: Option<[f32; 16]>,
}

pub struct GlbCamera {
    /// The aspec ratio of the camera (width / height)
    pub aspect_ratio: Option<f32>,
    /// The fov of the camara in radians
    pub fov_radian: Option<f32>,
    pub rotation: Option<[f32; 4]>,
    pub translation: Option<[f32; 3]>,
    pub scale: Option<[f32; 3]>,
    pub matrix: Option<[f32; 16]>,
}

pub struct GlbBufferViewPosition {
    start: u32,
    length: u32,
    accessory_type: AccessorType,
    component_type: ComponentType
}