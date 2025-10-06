use crate::{
    Authorization, DeliveryTimeout, MaxCacheDuration,
    modules::moqt::messages::control_messages::version_specific_parameters::VersionSpecificParameter,
};

pub(super) fn resolve_param(
    mut params: Vec<VersionSpecificParameter>,
) -> (
    Vec<Authorization>,
    Vec<DeliveryTimeout>,
    Vec<MaxCacheDuration>,
) {
    let authorization_info = params
        .iter_mut()
        .filter_map(|e| {
            if let VersionSpecificParameter::AuthorizationInfo(param) = e {
                Some(param.get_value())
            } else {
                None
            }
        })
        .collect();

    let delivery_timeout = params
        .iter_mut()
        .filter_map(|e| {
            if let VersionSpecificParameter::DeliveryTimeout(param) = e {
                Some(param.get_value())
            } else {
                None
            }
        })
        .collect();

    let max_cache_duration = params
        .iter_mut()
        .filter_map(|e| {
            if let VersionSpecificParameter::MaxCacheDuration(param) = e {
                Some(param.get_value())
            } else {
                None
            }
        })
        .collect();

    (authorization_info, delivery_timeout, max_cache_duration)
}
