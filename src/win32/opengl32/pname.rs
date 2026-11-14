//! How many values a parameter name has, for the functions that take
//! a name and a pointer to its values: glLightfv, glMaterialfv,
//! glLightModelfv, glFogfv, glTexEnvfv, glTexParameterfv, glTexGenfv
//! and their integer and double forms.
//!
//! OpenGL's names are numbers of one space, so one table has them all.
//! A handler reads as many values as the table says, after its own
//! check of which names its function takes, and a display list copies
//! as many when it compiles the call (`lists::copy`).

const GL_TEXTURE_BORDER_COLOR: u32 = 0x1004;
const GL_LIGHT_MODEL_AMBIENT: u32 = 0x0B53;
const GL_FOG_COLOR: u32 = 0x0B66;
const GL_AMBIENT: u32 = 0x1200;
const GL_POSITION: u32 = 0x1203;
const GL_SPOT_DIRECTION: u32 = 0x1204;
const GL_EMISSION: u32 = 0x1600;
const GL_AMBIENT_AND_DIFFUSE: u32 = 0x1602;
const GL_COLOR_INDEXES: u32 = 0x1603;
const GL_TEXTURE_ENV_COLOR: u32 = 0x2201;
const GL_OBJECT_PLANE: u32 = 0x2501;
const GL_EYE_PLANE: u32 = 0x2502;

/// The values of the parameter `pname`: four for a color, a position
/// or a plane, three for a direction or the color indexes, and one for
/// every other name.
pub fn values(pname: u32) -> usize {
    match pname {
        // GL_AMBIENT, GL_DIFFUSE, GL_SPECULAR and GL_POSITION are in a
        // row.
        GL_AMBIENT..=GL_POSITION
        | GL_EMISSION
        | GL_AMBIENT_AND_DIFFUSE
        | GL_LIGHT_MODEL_AMBIENT
        | GL_FOG_COLOR
        | GL_TEXTURE_ENV_COLOR
        | GL_TEXTURE_BORDER_COLOR
        | GL_OBJECT_PLANE
        | GL_EYE_PLANE => 4,
        GL_SPOT_DIRECTION | GL_COLOR_INDEXES => 3,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::values;

    #[test]
    fn colors_positions_and_planes_have_four_values() {
        // The light's colors and position, the material's, the lighting
        // model's ambient, the fog's and the texture environment's
        // colors, a texture's border and glTexGen's planes.
        for p in [
            0x1200, 0x1201, 0x1202, 0x1203, 0x1600, 0x1602, 0x0B53, 0x0B66, 0x2201, 0x1004, 0x2501,
            0x2502,
        ] {
            assert_eq!(values(p), 4, "{p:#X}");
        }
        // GL_SPOT_DIRECTION and GL_COLOR_INDEXES.
        assert_eq!((values(0x1204), values(0x1603)), (3, 3));
        // GL_SPOT_EXPONENT, GL_SHININESS, GL_FOG_MODE,
        // GL_TEXTURE_ENV_MODE, GL_TEXTURE_MIN_FILTER.
        for p in [0x1205, 0x1601, 0x0B65, 0x2200, 0x2801] {
            assert_eq!(values(p), 1, "{p:#X}");
        }
    }
}
