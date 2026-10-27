//! Uniform locations resolved at link time, with their last values.
//! Program uniforms survive program switches; each linked program owns
//! its own cache, recreated with the renderer after a context change.

use glow::HasContext;
use std::cell::Cell;

pub(super) trait Locations {
    fn resolve(gl: &glow::Context, program: glow::Program) -> Self;
}

pub(super) struct Uniform<T: Copy> {
    location: Option<glow::UniformLocation>,
    sent: Cell<Option<T>>,
}

impl<T: Value> Uniform<T> {
    /// Send `value` unless it is the one last sent.
    ///
    /// # Safety
    ///
    /// This uniform's program must be current on the renderer's context.
    /// Sent to another program, the value would also be taken as sent to
    /// this one, which would then never get it.
    pub unsafe fn set(&self, gl: &glow::Context, value: T) {
        if self.sent.get() != Some(value) {
            // SAFETY: the caller has made this uniform's program current.
            unsafe { value.upload(gl, self.location.as_ref()) };
            self.sent.set(Some(value));
        }
    }
}

pub(super) trait Value: Copy + PartialEq {
    unsafe fn upload(self, gl: &glow::Context, location: Option<&glow::UniformLocation>);
}

macro_rules! scalar {
    ($ty:ty, $call:ident) => {
        impl Value for $ty {
            unsafe fn upload(self, gl: &glow::Context, location: Option<&glow::UniformLocation>) {
                unsafe { gl.$call(location, self) };
            }
        }
    };
}
scalar!(i32, uniform_1_i32);
scalar!(f32, uniform_1_f32);

macro_rules! vector {
    ($ty:ty, $call:ident, [$($i:tt),+]) => {
        impl Value for $ty {
            unsafe fn upload(self, gl: &glow::Context, location: Option<&glow::UniformLocation>) {
                unsafe { gl.$call(location, $(self[$i]),+) };
            }
        }
    };
}
vector!([f32; 2], uniform_2_f32, [0, 1]);
vector!([f32; 3], uniform_3_f32, [0, 1, 2]);
vector!([f32; 4], uniform_4_f32, [0, 1, 2, 3]);
vector!([i32; 3], uniform_3_i32, [0, 1, 2]);
vector!([i32; 4], uniform_4_i32, [0, 1, 2, 3]);

// The fog tables (and a fastfill's dither pattern) are kept as the draw
// state has them, so a draw compares them without converting; they
// become ints only when they are sent.
macro_rules! table {
    ($ty:ty, $len:literal) => {
        impl Value for [$ty; $len] {
            unsafe fn upload(self, gl: &glow::Context, location: Option<&glow::UniformLocation>) {
                unsafe { gl.uniform_1_i32_slice(location, &self.map(i32::from)) };
            }
        }
    };
}
table!(u8, 64);
table!(i16, 64);
table!(u16, 16);

macro_rules! locations {
    ($name:ident { $($field:ident: $ty:ty),* $(,)? }) => {
        pub(super) struct $name {
            $(pub $field: Uniform<$ty>),*
        }
        impl Locations for $name {
            fn resolve(gl: &glow::Context, program: glow::Program) -> Self {
                // SAFETY: the program has just been linked on this context.
                unsafe {
                    Self {
                        $($field: Uniform {
                            location: gl.get_uniform_location(program, stringify!($field))
                                .or_else(|| gl.get_uniform_location(
                                    program, concat!(stringify!($field), "[0]")
                                )),
                            sent: Cell::new(None),
                        }),*
                    }
                }
            }
        }
    };
}

locations!(Triangle {
    u_size: [f32; 2],
    u_fbzcp: i32,
    u_fbz: i32,
    u_alpha: i32,
    u_fog: i32,
    u_color0: i32,
    u_color1: i32,
    u_chroma: i32,
    u_chroma_range: i32,
    u_zacolor: i32,
    u_fogcolor: i32,
    u_stipple: i32,
    u_yorigin: i32,
    u_fogblend: [u8; 64],
    u_fogdelta: [i16; 64],
    u_scale: f32,
    u_units: i32,
    u_config: i32,
    u_tmode0: i32,
    u_tmode1: i32,
    u_tsize0: [f32; 3],
    u_tsize1: [f32; 3],
    u_tlod0: [i32; 3],
    u_tlod1: [i32; 3],
    u_tdetail0: [i32; 3],
    u_tdetail1: [i32; 3],
    u_constant_depth: i32,
    u_dither: i32
});
locations!(Quad {
    u_rect: [f32; 4],
    u_size: [f32; 2],
    u_scale: f32
});
locations!(FillQuad {
    u_rect: [f32; 4],
    u_size: [f32; 2],
    u_scale: f32,
    u_pattern: [u16; 16]
});
locations!(Banshee {
    u_rect: [f32; 4],
    u_size: [f32; 2],
    u_scale: i32,
    u_across: i32,
    u_desk: [i32; 4],
    u_desk_layout: [i32; 4],
    u_ovl: [i32; 4],
    u_ovl_layout: [i32; 4],
    u_ovl_rect: [i32; 4],
    u_ovl_step: [i32; 3],
    u_cur: [i32; 4],
    u_cur_c0: [f32; 3],
    u_cur_c1: [f32; 3]
});
