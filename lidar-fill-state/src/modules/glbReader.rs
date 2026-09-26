/// The Size of the GLB. After the header the chunks start.
pub const GLB_Header_Size: u8 = 12;

/// Checks the magic_number if it is a glb
/// 
/// ### Parameters:
/// 
/// **file_buffer**: Buffer of the start of the file
pub fn is_glb_file(file_buffer: &[u8]) -> bool {
    if file_buffer.len() < 4  {
        false
    } else {
        true
    }
}