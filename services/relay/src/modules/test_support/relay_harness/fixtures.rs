pub(crate) mod cached_object;
pub(crate) mod data_object;
pub(crate) mod disk_cache;
pub(crate) mod subscription;

use crate::modules::data_plane::cache::track_cache::PlannedFetch;
pub(crate) use crate::modules::data_plane::cache::track_cache::location;

/// The plan as Ok(object location) and Err(last position) for each unknown
/// stretch.
pub(crate) fn planned_steps(plan: Vec<PlannedFetch>) -> Vec<Result<(u64, u64), (u64, u64)>> {
    plan.into_iter()
        .map(|step| match step {
            PlannedFetch::Object(object) => Ok((
                object.object.location.group_id,
                object.object.location.object_id,
            )),
            PlannedFetch::Unknown { last } => Err((last.group_id, last.object_id)),
        })
        .collect()
}
