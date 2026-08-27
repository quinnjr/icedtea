//! The CSS painter.

pub mod geometry;

pub use geometry::{
    Side, clamp_radii, inner_radii, rounded_rect_path, rounded_ring_path, side_wedge_path,
};
