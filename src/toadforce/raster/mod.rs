//! The software renderer: level 1's draws into card memory ("The
//! software renderer" in `docs/toadforce.md`). It is the definition of
//! what the card draws, which the OpenGL renderer is tested against.
//!
//! A draw goes through these, in this order:
//!
//! - `vertex`: the vertex layout a draw names, and a vertex in screen
//!   space.
//! - `pipeline`: the lowering of the card's state to the steps a
//!   fragment goes through, with the numbers and textures of the draw
//!   beside them.
//! - `sample` and `texel`: a texture's colour at a coordinate, read
//!   from card memory, in every format of the level.
//! - `setup`: the pixels a triangle covers and its vertices' values at
//!   each.
//! - `combine`: the arithmetic of the stages, the tests and the blend.
//! - `mem` and `pool`: card memory as several threads use it, and the
//!   threads.
//! - `math`: the logarithm and exponential the above need, the same on
//!   every host.
//!
//! Still to come, with the draw commands: the set-up of lines and
//! points, the code that runs the steps over the pixels of a row, and
//! the queue that hands rows to the threads.

pub mod combine;
pub mod math;
pub mod mem;
pub mod pipeline;
pub mod pool;
pub mod sample;
pub mod setup;
pub mod texel;
pub mod vertex;
