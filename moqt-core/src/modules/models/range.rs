#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    start: Option<Start>,
    end: Option<End>,
}

impl Range {
    pub fn new(
        start_group: Option<u64>,
        start_object: Option<u64>,
        end_group: Option<u64>,
        end_object: Option<u64>,
    ) -> Self {
        let start = match (start_group, start_object) {
            (Some(group_id), Some(object_id)) => Some(Start::new(group_id, object_id)),
            _ => None,
        };

        let end = match (end_group, end_object) {
            (Some(group_id), Some(object_id)) => Some(End::new(group_id, object_id)),
            _ => None,
        };

        // TODO: Validate that start is before end

        Self { start, end }
    }

    pub fn start_group(&self) -> Option<u64> {
        self.start.as_ref()?;
        Some(self.start.as_ref().unwrap().group_id())
    }

    pub fn start_object(&self) -> Option<u64> {
        self.start.as_ref()?;
        Some(self.start.as_ref().unwrap().object_id())
    }

    pub fn end_group(&self) -> Option<u64> {
        self.end.as_ref()?;
        Some(self.end.as_ref().unwrap().group_id())
    }

    pub fn end_object(&self) -> Option<u64> {
        self.end.as_ref()?;
        Some(self.end.as_ref().unwrap().object_id())
    }

    pub fn is_end(&self, group_id: u64, object_id: u64) -> bool {
        if self.end.is_some() {
            self.end.as_ref().unwrap().is_end(group_id, object_id)
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    group_id: u64,
    object_id: u64,
}

impl Start {
    pub fn new(group_id: u64, object_id: u64) -> Self {
        Self {
            group_id,
            object_id,
        }
    }

    pub fn group_id(&self) -> u64 {
        self.group_id
    }

    pub fn object_id(&self) -> u64 {
        self.object_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct End {
    group_id: u64,
    object_id: u64,
}

impl End {
    pub fn new(group_id: u64, object_id: u64) -> Self {
        Self {
            group_id,
            object_id,
        }
    }

    pub fn group_id(&self) -> u64 {
        self.group_id
    }

    pub fn object_id(&self) -> u64 {
        self.object_id
    }

    pub fn is_end(&self, group_id: u64, object_id: u64) -> bool {
        self.group_id == group_id && self.object_id == object_id
    }
}
