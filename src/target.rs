use anyhow::{anyhow, Result};
use pulse::volume::{ChannelVolumes, Volume};
use pulsectl::controllers::{
    types::ApplicationInfo, AppControl, DeviceControl, SinkController, SourceController,
};

#[derive(Clone)]
pub enum Target {
    StaticSink(u32),
    StaticSource(u32),
    SinkWithProperty(&'static str, &'static str),
    Any(Vec<Target>),
    All(Vec<Target>),
}

impl Target {
    pub fn volume(
        &self,
        sink: &mut SinkController,
        source: &mut SourceController,
    ) -> Result<Option<Volume>> {
        match self {
            Target::StaticSink(idx) => Ok(Some(sink.get_device_by_index(*idx)?.volume.avg())),
            Target::StaticSource(idx) => Ok(Some(source.get_device_by_index(*idx)?.volume.avg())),
            Target::SinkWithProperty(p, v) => {
                Ok(Self::find_app(p, v, sink)?.map(|a| a.volume.avg()))
            }
            Target::Any(targets) => {
                for t in targets {
                    if let Some(v) = t.volume(sink, source)? {
                        return Ok(Some(v));
                    }
                }
                Ok(None)
            }
            Target::All(_) => todo!(),
        }
    }

    pub fn set_volume(
        &self,
        sink: &mut SinkController,
        source: &mut SourceController,
        new_vol: f32,
    ) -> Result<Option<()>> {
        match self {
            Target::StaticSink(idx) => {
                let mut vol = sink.get_device_by_index(*idx)?.volume;
                vol.set(
                    vol.len(),
                    Volume((new_vol * (Volume::NORMAL.0 - 1) as f32) as u32),
                );
                sink.set_device_volume_by_index(*idx, &vol);
                Ok(Some(()))
            }
            Target::StaticSource(idx) => {
                let mut vol = sink.get_app_by_index(*idx)?.volume;
                vol.set(
                    vol.len(),
                    Volume((new_vol * (Volume::NORMAL.0 - 1) as f32) as u32),
                );
                sink.set_sink_input_volume(*idx, &vol)?;
                Ok(Some(()))
            }
            Target::SinkWithProperty(p, v) => {
                if let Some(app) = Self::find_app(p, v, sink)? {
                    let mut vol = app.volume;
                    vol.set(
                        vol.len(),
                        Volume((new_vol * (Volume::NORMAL.0 - 1) as f32) as u32),
                    );
                    sink.set_sink_input_volume(app.index, &vol)?;
                    Ok(Some(()))
                } else {
                    Ok(None)
                }
            }
            Target::Any(targets) => {
                for t in targets {
                    if let Some(v) = t.set_volume(sink, source, new_vol)? {
                        return Ok(Some(v));
                    }
                }
                Ok(None)
            }
            Target::All(_) => todo!(),
        }
    }

    pub fn muted(
        &self,
        sink: &mut SinkController,
        source: &mut SourceController,
    ) -> Result<Option<bool>> {
        match self {
            Target::StaticSink(idx) => Ok(Some(sink.get_device_by_index(*idx).unwrap().mute)),
            Target::StaticSource(idx) => Ok(Some(source.get_device_by_index(*idx).unwrap().mute)),
            Target::SinkWithProperty(p, v) => {
                Ok(Self::find_app(p, v, sink).unwrap().map(|a| a.mute))
            }
            Target::Any(targets) => {
                for t in targets {
                    if let Some(v) = t.muted(sink, source).unwrap() {
                        return Ok(Some(v));
                    }
                }
                Ok(None)
            }
            Target::All(_) => todo!(),
        }
    }

    pub fn toggle_muted(
        &self,
        sink: &mut SinkController,
        source: &mut SourceController,
    ) -> Result<Option<()>> {
        match self {
            Target::StaticSink(idx) => {
                let is_muted = sink.get_device_by_index(*idx)?.mute;
                sink.set_device_mute_by_index(*idx, !is_muted);

                Ok(Some(()))
            }
            Target::StaticSource(idx) => {
                let is_muted = source.get_device_by_index(*idx)?.mute;
                source.set_device_mute_by_index(*idx, !is_muted);
                Ok(Some(()))
            }
            Target::SinkWithProperty(p, v) => {
                if let Some(app) = Self::find_app(p, v, sink)? {
                    let idx = app.index;
                    let is_muted = sink.get_app_by_index(idx)?.mute;
                    sink.set_app_mute(idx, !is_muted)?;
                    Ok(Some(()))
                } else {
                    Ok(None)
                }
            }
            Target::Any(targets) => {
                for t in targets {
                    if let Some(v) = t.toggle_muted(sink, source)? {
                        return Ok(Some(v));
                    }
                }
                Ok(None)
            }
            Target::All(_) => todo!(),
        }
    }

    pub fn selected(
        &self,
        sink: &mut SinkController,
        source: &mut SourceController,
    ) -> Result<Option<bool>> {
        match self {
            Target::StaticSink(idx) => Ok(Some(sink.get_default_device()?.index == *idx)),
            Target::StaticSource(idx) => Ok(Some(source.get_default_device()?.index == *idx)),
            _ => Err(anyhow!(
                "Only StaticSink/StaticSource can be used for DefaultSelect bindings"
            )),
        }
    }

    pub fn set_as_selected(
        &self,
        sink: &mut SinkController,
        source: &mut SourceController,
    ) -> Result<Option<()>> {
        match self {
            Target::StaticSink(idx) => {
                let name = sink.get_device_by_index(*idx)?.name.ok_or_else(|| {
                    anyhow!("Device must have a name to be set as default output")
                })?;
                sink.set_default_device(&name)?;
                Ok(Some(()))
            }
            Target::StaticSource(idx) => {
                let name = source
                    .get_device_by_index(*idx)?
                    .name
                    .ok_or_else(|| anyhow!("Device must have a name to be set as default input"))?;
                source.set_default_device(&name)?;
                Ok(Some(()))
            }
            _ => Err(anyhow!(
                "Only StaticSink/StaticSource can be used for DefaultSelect bindings"
            )),
        }
    }
}

impl Target {
    fn find_app(
        property: &str,
        value: &str,
        sink: &mut SinkController,
    ) -> Result<Option<ApplicationInfo>> {
        let apps = sink.list_applications()?;
        Ok(apps.into_iter().find(|app| {
            app.proplist
                .get_str(property)
                .filter(|v| value == v)
                .is_some()
        }))
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
