//! Stage 1: photo -> hole-accurate leg positions.

use super::Photo;
use super::fixtures::Fixture;
use super::types::GridMap;
use anyhow::Result;

pub trait GridMapper: Send {
    fn name(&self) -> String;
    fn map(&self, photo: &Photo) -> Result<GridMap>;
}

/// Placeholder until the OpenCV mapper in `vision/` exists: ignores the photo
/// and returns a demo circuit's hand-made grid map.
pub struct FixtureGridMapper {
    pub fixture: Fixture,
}

impl GridMapper for FixtureGridMapper {
    fn name(&self) -> String {
        format!("stub: {}", self.fixture.name)
    }

    fn map(&self, _photo: &Photo) -> Result<GridMap> {
        Ok(self.fixture.grid.clone())
    }
}
