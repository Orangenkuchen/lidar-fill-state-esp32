const ROWS: usize = 8;
const COLS: usize = 8;

const GRID_VERTICES: usize = ROWS * COLS;
const PERIMETER_VERTICES: usize = 2 * ROWS + 2 * COLS - 4;

const ORIGINAL_VERTICES: usize =
    GRID_VERTICES + PERIMETER_VERTICES;

const TOTAL_VERTICES: usize =
    ORIGINAL_VERTICES * 2;

// 98 grid triangles + 56 border triangles
const ORIGINAL_TRIANGLES: usize =
    (ROWS - 1) * (COLS - 1) * 2
    + PERIMETER_VERTICES * 2;

// Original surface + bottom + side walls
const TOTAL_TRIANGLES: usize =
    ORIGINAL_TRIANGLES * 2 + PERIMETER_VERTICES * 2;

const TOTAL_INDICES: usize = TOTAL_TRIANGLES * 3;

pub struct LidarMesh {
    pub positions: [[f32; 3]; TOTAL_VERTICES],
    pub uvs: [[f32; 2]; TOTAL_VERTICES],
    pub indices: [u16; TOTAL_INDICES],
    pub vertex_count: usize,
    pub index_count: usize,
}

impl LidarMesh {
    pub const fn new() -> Self {
        Self {
            positions: [[0.0; 3]; TOTAL_VERTICES],
            uvs: [[0.0; 2]; TOTAL_VERTICES],
            indices: [0; TOTAL_INDICES],
            vertex_count: 0,
            index_count: 0,
        }
    }

    fn push_vertex(
        &mut self,
        position: [f32; 3],
        uv: [f32; 2],
    ) -> usize {
        let index = self.vertex_count;

        self.positions[index] = position;
        self.uvs[index] = uv;

        self.vertex_count += 1;
        index
    }

    fn push_triangle(
        &mut self,
        a: usize,
        b: usize,
        c: usize,
    ) {
        self.indices[self.index_count] = a as u16;
        self.indices[self.index_count + 1] = b as u16;
        self.indices[self.index_count + 2] = c as u16;

        self.index_count += 3;
    }
}

/// Reproduces the TypeScript CreateLidarMesh() for an 8x8 sensor.
///
/// FOV arguments intentionally follow the original TypeScript
/// argument ordering.
///
/// The TypeScript calls:
/// CreateLidarMesh(8, 8, vertical_fov, horizontal_fov, ...)
///
/// Therefore fov_x receives vertical_fov.
pub fn create_lidar_mesh(
    readings_mm: &[i16; 64],
    vertical_fov_radians: f32,
    horizontal_fov_radians: f32,
    border_width: f32,
    extrusion_depth: f32,
) -> LidarMesh {
    let mut mesh = LidarMesh::new();

    let fov_x = horizontal_fov_radians;

    let fov_y = vertical_fov_radians;

    // --------------------------------------------------
    // 1. Generate LiDAR grid vertices
    // --------------------------------------------------

    for row in 0..ROWS {
        for col in 0..COLS {
            let theta =
                ((col as f32 + 0.5) / COLS as f32 - 0.5)
                * fov_x;

            let phi =
                (0.5 - (row as f32 + 0.5) / ROWS as f32)
                * fov_y;

            let d =
                readings_mm[row * COLS + col] as f32 / 1000.0;

            let x = d * libm::tanf(theta);
            let y = d * libm::tanf(phi);
            let z = -d;

            let u = col as f32 / (COLS - 1) as f32;
            let v = 1.0 - row as f32 / (ROWS - 1) as f32;

            mesh.push_vertex([x, y, z], [u, v]);
        }
    }

    // --------------------------------------------------
    // 2. Connect grid cells
    // --------------------------------------------------

    for row in 0..ROWS - 1 {
        for col in 0..COLS - 1 {
            let a = row * COLS + col;
            let b = a + 1;
            let c = a + COLS;
            let d = c + 1;

            mesh.push_triangle(a, c, b);
            mesh.push_triangle(b, c, d);
        }
    }

    // --------------------------------------------------
    // 3. Calculate grid center
    // --------------------------------------------------

    if border_width > 0.0 {
        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;

        for position in &mesh.positions[..mesh.vertex_count] {
            min_x = min_x.min(position[0]);
            max_x = max_x.max(position[0]);

            min_y = min_y.min(position[1]);
            max_y = max_y.max(position[1]);
        }

        let center_x = (min_x + max_x) * 0.5;
        let center_y = (min_y + max_y) * 0.5;

        // --------------------------------------------------
        // 4. Collect perimeter vertices in TypeScript order
        // --------------------------------------------------

        let mut perimeter = [0usize; PERIMETER_VERTICES];
        let mut perimeter_count = 0;

        // First row
        for col in 0..COLS {
            perimeter[perimeter_count] = col;
            perimeter_count += 1;
        }

        // Right edge
        for row in 1..ROWS {
            perimeter[perimeter_count] =
                row * COLS + COLS - 1;
            perimeter_count += 1;
        }

        // Last row
        for col in (0..COLS - 1).rev() {
            perimeter[perimeter_count] =
                (ROWS - 1) * COLS + col;
            perimeter_count += 1;
        }

        // Left edge
        for row in (1..ROWS - 1).rev() {
            perimeter[perimeter_count] = row * COLS;
            perimeter_count += 1;
        }

        // --------------------------------------------------
        // 5. Generate outer perimeter
        // --------------------------------------------------

        let mut outer_perimeter = [0usize; PERIMETER_VERTICES];

        for i in 0..perimeter_count {
            let index = perimeter[i];
            let position = mesh.positions[index];

            let dx = position[0] - center_x;
            let dy = position[1] - center_y;

            let length = libm::sqrtf(dx * dx + dy * dy);

            let (nx, ny) = if length > 1e-8 {
                (dx / length, dy / length)
            } else {
                (0.0, 0.0)
            };

            let outer_x = position[0] + nx * border_width;
            let outer_y = position[1] + ny * border_width;

            let uv = mesh.uvs[index];

            let outer_uv = [
                uv[0] + (outer_x - position[0]),
                uv[1] - (outer_y - position[1]),
            ];

            outer_perimeter[i] = mesh.push_vertex(
                [outer_x, outer_y, position[2]],
                outer_uv,
            );
        }

        // --------------------------------------------------
        // 6. Connect inner and outer perimeter
        // --------------------------------------------------

        for i in 0..perimeter_count {
            let next = (i + 1) % perimeter_count;

            let inner_a = perimeter[i];
            let inner_b = perimeter[next];

            let outer_a = outer_perimeter[i];
            let outer_b = outer_perimeter[next];

            mesh.push_triangle(inner_a, inner_b, outer_a);
            mesh.push_triangle(inner_b, outer_b, outer_a);
        }
    }

    // --------------------------------------------------
    // 7. Extrude entire mesh downwards
    // --------------------------------------------------

    if extrusion_depth > 0.0 {
        let original_vertex_count = mesh.vertex_count;
        let original_index_count = mesh.index_count;

        // Duplicate vertices
        for i in 0..original_vertex_count {
            let position = mesh.positions[i];
            let uv = mesh.uvs[i];

            mesh.push_vertex(
                [
                    position[0],
                    position[1],
                    position[2] - extrusion_depth,
                ],
                uv,
            );
        }

        // Duplicate triangles with reversed winding
        for i in (0..original_index_count).step_by(3) {
            let a = mesh.indices[i] as usize;
            let b = mesh.indices[i + 1] as usize;
            let c = mesh.indices[i + 2] as usize;

            mesh.push_triangle(
                a + original_vertex_count,
                c + original_vertex_count,
                b + original_vertex_count,
            );
        }

        // --------------------------------------------------
        // 8. Generate side walls
        // --------------------------------------------------

        let mut perimeter = [0usize; PERIMETER_VERTICES];
        let mut perimeter_count = 0;

        // First row
        for col in 0..COLS {
            perimeter[perimeter_count] = col;
            perimeter_count += 1;
        }

        // Right edge
        for row in 1..ROWS {
            perimeter[perimeter_count] =
                row * COLS + COLS - 1;
            perimeter_count += 1;
        }

        // Last row
        for col in (0..COLS - 1).rev() {
            perimeter[perimeter_count] =
                (ROWS - 1) * COLS + col;
            perimeter_count += 1;
        }

        // Left edge
        for row in (1..ROWS - 1).rev() {
            perimeter[perimeter_count] = row * COLS;
            perimeter_count += 1;
        }

        // Actual outer boundary
        let border_start = GRID_VERTICES;

        for i in 0..perimeter_count {
            perimeter[i] = border_start + i;
        }

        for i in 0..perimeter_count {
            let next = (i + 1) % perimeter_count;

            let a = perimeter[i];
            let b = perimeter[next];

            let bottom_a = a + original_vertex_count;
            let bottom_b = b + original_vertex_count;

            mesh.push_triangle(a, b, bottom_a);
            mesh.push_triangle(b, bottom_b, bottom_a);
        }
    }

    mesh
}