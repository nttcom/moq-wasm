#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRange {
    start: Option<ObjectStart>,
    end: Option<ObjectEnd>,
}

impl ObjectRange {
    pub fn new(
        start_group: Option<u64>,
        start_object: Option<u64>,
        end_group: Option<u64>,
        end_object: Option<u64>,
    ) -> Self {
        let start = match (start_group, start_object) {
            (Some(group_id), Some(object_id)) => Some(ObjectStart::new(group_id, object_id)),
            _ => None,
        };

        let end = end_group.map(|group_id| ObjectEnd::new(group_id, end_object));

        // TODO: Validate that start is before end

        Self { start, end }
    }

    pub fn start_group_id(&self) -> Option<u64> {
        let start = self.start.as_ref()?;
        Some(start.group_id())
    }

    pub fn start_object_id(&self) -> Option<u64> {
        let start = self.start.as_ref()?;
        Some(start.object_id())
    }

    pub fn end_group_id(&self) -> Option<u64> {
        let end = self.end.as_ref()?;
        Some(end.group_id())
    }

    pub fn end_object_id(&self) -> Option<u64> {
        let end = self.end.as_ref()?;
        end.object_id()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectStart {
    group_id: u64,
    object_id: u64,
}

impl ObjectStart {
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
pub struct ObjectEnd {
    group_id: u64,
    object_id: Option<u64>,
}

impl ObjectEnd {
    pub fn new(group_id: u64, object_id: Option<u64>) -> Self {
        Self {
            group_id,
            object_id,
        }
    }

    pub fn group_id(&self) -> u64 {
        self.group_id
    }

    pub fn object_id(&self) -> Option<u64> {
        self.object_id
    }
}
