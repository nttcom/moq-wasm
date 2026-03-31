use std::sync::Arc;

use crate::modules::core::data_object::DataObject;

#[derive(Clone, Debug)]
pub(crate) enum EgressMsg {
    Object {
        group_id: u64,
        data: Arc<DataObject>,
    },
    Close {
        group_id: u64,
    },
}
