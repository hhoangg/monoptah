//! The graph canvas of the workflow pages: the run page draws a run on it read-only, and the
//! board editor draws a workflow on it. `geometry` is the pure maths (view, fit, edge paths),
//! `scene` lays the pure board and run models out on the canvas, `canvas` is the pannable and
//! zoomable view and `cards` draws what sits on it.
mod canvas;
mod cards;
mod geometry;
#[cfg(test)]
mod geometry_tests;
mod hit;
#[cfg(test)]
mod hit_tests;
mod scene;
#[cfg(test)]
mod scene_tests;

pub(in crate::monos) use canvas::{Connector, GraphCanvas, GraphEvent};
pub(in crate::monos) use geometry::Rect;
pub(in crate::monos) use scene::{Bar, BarKind, Scene, TabItem};
