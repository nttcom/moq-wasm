use thiserror::Error;

/// The field of an object that the session's draft cannot carry: a draft-18
/// value of 2^62 or more sent on draft-14, whose varints stop below it
/// (draft-14 §1.4). Nothing of the object was written. A send that fails for
/// this reason has it in its error chain (`UnencodableObject::in_chain`).
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnencodableObject {
    /// The Group ID or the Object ID. Every later object of the group
    /// fails the same way when the Group ID does.
    #[error("Group ID {group_id} or Object ID {object_id} exceeds a draft-14 varint")]
    Location { group_id: u64, object_id: u64 },
    /// The Subgroup ID of a subgroup stream or a FETCH object. Only that
    /// subgroup fails; other subgroups can still be sent.
    #[error("Subgroup ID {subgroup_id} exceeds a draft-14 varint")]
    SubgroupId { subgroup_id: u64 },
    /// A key or value of the object's extension headers. Only this object
    /// fails; the next one of the same subgroup can still be sent.
    #[error("extension header {key} exceeds a draft-14 varint")]
    ExtensionHeaders { key: u64 },
}

impl UnencodableObject {
    pub fn in_chain(error: &anyhow::Error) -> Option<Self> {
        error
            .chain()
            .find_map(|cause| cause.downcast_ref::<Self>())
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Context;

    use super::UnencodableObject;

    #[test]
    fn finds_the_field_through_context_layers() {
        // Arrange
        let unencodable = Err::<(), _>(UnencodableObject::SubgroupId {
            subgroup_id: 1 << 62,
        })
        .context("send subgroup header")
        .unwrap_err();
        let other = anyhow::anyhow!("the current object's payload lacks 3 bytes");

        // Act / Assert
        assert_eq!(
            UnencodableObject::in_chain(&unencodable),
            Some(UnencodableObject::SubgroupId {
                subgroup_id: 1 << 62
            })
        );
        assert_eq!(UnencodableObject::in_chain(&other), None);
    }
}
