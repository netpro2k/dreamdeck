use crate::target::Target;

pub enum Binding {
    VolumeControl(Target),
    MuteToggle(Target),
    DefaultSelect(Target),
}

impl Binding {
    pub fn volume(t: Target) -> Binding {
        Self::VolumeControl(t)
    }
    pub fn mute(t: Target) -> Binding {
        Self::MuteToggle(t)
    }
    pub fn select(t: Target) -> Binding {
        Self::DefaultSelect(t)
    }

    pub fn to_mute(&self) -> Self {
        let t = match self {
            Binding::VolumeControl(t) => t,
            Binding::MuteToggle(t) => t,
            Binding::DefaultSelect(t) => t,
        };
        Self::MuteToggle(t.clone())
    }

    #[allow(dead_code)]
    pub fn to_volume(&self) -> Self {
        let t = match self {
            Binding::VolumeControl(t) => t,
            Binding::MuteToggle(t) => t,
            Binding::DefaultSelect(t) => t,
        };
        Self::VolumeControl(t.clone())
    }
}
