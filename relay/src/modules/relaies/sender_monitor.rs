use crate::modules::{
    enums::{FilterType, GroupOrder},
    relaies::caches::latest_info::LatestInfo,
};

pub(crate) enum SubscribeInfoReceiver {
    FilterTypeUpdate(FilterType),
    GroupOrderUpdate(GroupOrder),
}

pub(crate) struct SenderMonitor {
    join_handle: tokio::task::JoinHandle<()>,
}

impl SenderMonitor {
    pub(crate) async fn run(
        mut group_order: GroupOrder,
        mut filter_type: FilterType,
        mut subscribe_info_receiver: tokio::sync::broadcast::Receiver<SubscribeInfoReceiver>,
        mut info_receiver: tokio::sync::broadcast::Receiver<LatestInfo>,
        mut sender: tokio::sync::mpsc::Sender<(u64, u64)>, // group_id, offset
    ) -> Self {
        let mut offset = 0;
        let mut incrementer = match group_order {
            GroupOrder::Ascending | GroupOrder::Publisher => 1,
            GroupOrder::Descending => -1,
        };
        let join_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    Ok(subscribe_info) = subscribe_info_receiver.recv() => {
                        match subscribe_info {
                            SubscribeInfoReceiver::FilterTypeUpdate(new_filter_type) => {
                                filter_type = new_filter_type;
                            }
                            SubscribeInfoReceiver::GroupOrderUpdate(new_group_order) => {
                                group_order = new_group_order
                            }
                        }
                    }
                    Ok(latest_info) = info_receiver.recv() => {
                        sender.send((latest_info.group_id, offset)).await.unwrap();
                    }
                }
            }
        });
        SenderMonitor { join_handle }
    }
}
