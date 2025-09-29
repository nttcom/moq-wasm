use crate::modules::namespace_table::NamespaceTable;

type PublisherId = usize;
type SubscriberId = usize;
type Namespace = String;

pub(crate) struct MessageHandler;

impl MessageHandler {
    pub(crate) fn publish_namespace(
        sub_table: &mut NamespaceTable,
        subscriber_id: SubscriberId,
        namespace: Namespace,
    ) {
        sub_table.add_namespace(namespace.clone());
        sub_table.add_id(namespace, subscriber_id);
    }

    pub(crate) fn subscribe_namespace(
        pub_table: &mut NamespaceTable,
        publisher_id: PublisherId,
        namespace: Namespace,
    ) {
        pub_table.add_namespace(namespace.clone());
        pub_table.add_id(namespace, publisher_id);
    }
}
