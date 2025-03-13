use crate::models::subscriptions::{fetch, subscribe};

pub enum Subscription {
    Subscribe(subscribe::Subscription),
    Fetch(fetch::Subscription),
}
