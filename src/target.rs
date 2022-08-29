use anyhow::{anyhow, Result};
use dyn_clonable::*;
use pulse::volume::ChannelVolumes;
use pulsectl::controllers::{
    types::{ApplicationInfo, DeviceInfo},
    AppControl, DeviceControl, SinkController, SourceController,
};

pub enum Target {
    DeviceSink(DeviceInfo),
    AppSink(ApplicationInfo),
    DeviceSource(DeviceInfo),
    // AppSource(ApplicationInfo),
}
impl Target {
    pub fn is_muted(&self) -> bool {
        match self {
            Target::DeviceSink(device) => device.mute,
            Target::DeviceSource(device) => device.mute,
            Target::AppSink(app) => app.mute,
        }
    }
    pub fn volume(&self) -> ChannelVolumes {
        match self {
            Target::DeviceSink(device) => device.volume,
            Target::DeviceSource(device) => device.volume,
            Target::AppSink(app) => app.volume,
        }
    }
}

pub type TargetableResult = Result<Option<Target>>;

#[clonable]
pub trait Targetable: Clone {
    fn get_target(
        &self,
        sink_controller: &mut SinkController,
        source_controller: &mut SourceController,
    ) -> TargetableResult;
}

#[derive(Clone)]
pub struct StaticSink(u32);
impl Targetable for StaticSink {
    fn get_target(
        &self,
        sink_controller: &mut SinkController,
        _source_controller: &mut SourceController,
    ) -> TargetableResult {
        let device = sink_controller.get_device_by_index(self.0)?;
        Ok(Some(Target::DeviceSink(device)))
    }
}
impl From<&DeviceInfo> for StaticSink {
    fn from(d: &DeviceInfo) -> Self {
        Self(d.index)
    }
}

#[derive(Clone)]
pub struct StaticSource(u32);
impl Targetable for StaticSource {
    fn get_target(
        &self,
        _sink_controller: &mut SinkController,
        source_controller: &mut SourceController,
    ) -> TargetableResult {
        let device = source_controller.get_device_by_index(self.0)?;
        Ok(Some(Target::DeviceSource(device)))
    }
}
impl From<&DeviceInfo> for StaticSource {
    fn from(d: &DeviceInfo) -> Self {
        Self(d.index)
    }
}

#[derive(Clone)]
pub struct SinkWithProperty<'a>(&'a str, &'a str);
impl SinkWithProperty<'_> {
    fn find_app(&self, sink_controller: &mut SinkController) -> Result<Option<ApplicationInfo>> {
        let apps = sink_controller.list_applications()?;
        Ok(apps.into_iter().find(|app| {
            app.proplist
                .get_str(self.0)
                .filter(|v| self.1 == v)
                .is_some()
        }))
    }
    pub fn app_name(name: &'static str) -> Box<Self> {
        Box::new(SinkWithProperty(
            pulse::proplist::properties::APPLICATION_NAME,
            name,
        ))
    }
    pub fn process_binary(name: &'static str) -> Box<Self> {
        Box::new(SinkWithProperty(
            pulse::proplist::properties::APPLICATION_PROCESS_BINARY,
            name,
        ))
    }
    pub fn media_name(name: &'static str) -> Box<Self> {
        Box::new(SinkWithProperty(
            pulse::proplist::properties::MEDIA_NAME,
            name,
        ))
    }
}
impl Targetable for SinkWithProperty<'_> {
    fn get_target(
        &self,
        sink_controller: &mut SinkController,
        _source_controller: &mut SourceController,
    ) -> TargetableResult {
        let app = self.find_app(sink_controller)?;
        Ok(app.map(|app| Target::AppSink(app)))
    }
}

#[derive(Clone)]
pub struct FirstValidTarget(Vec<Box<dyn Targetable>>);
impl FirstValidTarget {
    pub fn new(t: Vec<Box<dyn Targetable>>) -> Self {
        FirstValidTarget(t)
    }
}
impl Targetable for FirstValidTarget {
    fn get_target(
        &self,
        sink_controller: &mut SinkController,
        source_controller: &mut SourceController,
    ) -> TargetableResult {
        // We want to get the first non-None target but still propagate errors up
        let first_valid = self
            .0
            .iter()
            .map(|g| g.get_target(sink_controller, source_controller))
            .filter(|g| g.is_err() || g.as_ref().unwrap().is_some())
            .next();
        match first_valid {
            Some(r) => r,
            None => Ok(None),
        }
    }
}

pub trait SinkControllerExt {
    fn set_sink_input_volume(&mut self, index: u32, vol: &ChannelVolumes) -> Result<()>;
}
impl SinkControllerExt for SinkController {
    fn set_sink_input_volume(&mut self, index: u32, vol: &ChannelVolumes) -> Result<()> {
        let op = self
            .handler
            .introspect
            .set_sink_input_volume(index, vol, None);
        self.handler
            .wait_for_operation(op)
            .map_err(|_| anyhow!("Failed to set sink input volume"))
    }
}

#[derive(Clone)]
pub struct AlwaysNone {}
impl Targetable for AlwaysNone {
    fn get_target(
        &self,
        _sink_controller: &mut SinkController,
        _source_controller: &mut SourceController,
    ) -> TargetableResult {
        return Ok(None);
    }
}

#[derive(Clone)]
pub struct AlwaysError {}
impl Targetable for AlwaysError {
    fn get_target(
        &self,
        _sink_controller: &mut SinkController,
        _source_controller: &mut SourceController,
    ) -> TargetableResult {
        return Err(anyhow!("AlwaysError always errors"));
    }
}
