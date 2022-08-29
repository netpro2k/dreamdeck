use crate::Targetable;

pub enum Binding {
    VolumeControl(Box<dyn Targetable>),
    MuteToggle(Box<dyn Targetable>),
    DefaultSelect(Box<dyn Targetable>),
}
impl Binding {
    pub fn volume<T: Targetable + 'static>(t: T) -> Binding {
        Self::VolumeControl(Box::new(t))
    }
    pub fn mute<T: Targetable + 'static>(t: T) -> Binding {
        Self::MuteToggle(Box::new(t))
    }
    pub fn select<T: Targetable + 'static>(t: T) -> Binding {
        Self::DefaultSelect(Box::new(t))
    }

    pub fn to_mute(&self) -> Self {
        let v = match self {
            Binding::VolumeControl(t) => t,
            Binding::MuteToggle(t) => t,
            Binding::DefaultSelect(t) => t,
        };
        Self::MuteToggle(v.clone())
    }
}
