use moqt::object;
use pyo3::prelude::*;

/// A Location (draft-14 §1.4.1).
#[pyclass(frozen, eq, hash, get_all, from_py_object, module = "moq_wasm")]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Location {
    pub group_id: u64,
    pub object_id: u64,
}

#[pymethods]
impl Location {
    #[new]
    fn new(group_id: u64, object_id: u64) -> Self {
        Self {
            group_id,
            object_id,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "Location(group_id={}, object_id={})",
            self.group_id, self.object_id
        )
    }
}

impl From<object::Location> for Location {
    fn from(location: object::Location) -> Self {
        Self {
            group_id: location.group_id,
            object_id: location.object_id,
        }
    }
}

impl From<Location> for object::Location {
    fn from(location: Location) -> Self {
        Self {
            group_id: location.group_id,
            object_id: location.object_id,
        }
    }
}
