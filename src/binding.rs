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
}
impl Binding {
    pub fn mute<T: Targetable + 'static>(t: T) -> Binding {
        Self::MuteToggle(Box::new(t))
    }
}
impl Binding {
    pub fn select<T: Targetable + 'static>(t: T) -> Binding {
        Self::DefaultSelect(Box::new(t))
    }
}
