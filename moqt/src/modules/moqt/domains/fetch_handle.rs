use crate::{
    GroupOrder, Location,
    modules::moqt::control_plane::control_messages::messages::fetch_ok::FetchOk,
};

#[derive(Debug)]
pub struct FetchHandle {
    pub request_id: u64,
    pub group_order: GroupOrder,
    pub end_of_track: bool,
    pub end_location: Location,
}

impl FetchHandle {
    pub(crate) fn new(fetch_ok: FetchOk) -> Self {
        Self {
            request_id: fetch_ok.request_id,
            group_order: fetch_ok.group_order,
            end_of_track: fetch_ok.end_of_track,
            end_location: fetch_ok.end_location,
        }
    }
}
