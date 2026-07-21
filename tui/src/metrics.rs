#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PositionDirection {
    Waiting,
    Flat,
    Long,
    Short,
}

impl PositionDirection {
    pub fn from_position(position: Option<f64>) -> Self {
        match position {
            None => Self::Waiting,
            Some(value) if value > 0.0 => Self::Long,
            Some(value) if value < 0.0 => Self::Short,
            Some(_) => Self::Flat,
        }
    }
}
