use std::collections::HashMap;

use anyhow::{anyhow, Result};
use midir::MidiOutputConnection;
use pulse::volume::Volume;
use pulsectl::controllers::{AppControl, DeviceControl, SinkController, SourceController};

use crate::{binding::Binding, binding::Binding::*, target::*};

const KNOB_UPDATE: u8 = 0xBA;
const BTN_DOWN: u8 = 0x9A;
const BTN_UP: u8 = 0x8A;

pub struct Deck {
    sink: SinkController,
    source: SourceController,

    bindings: HashMap<u8, Binding>,

    midi_out: MidiOutputConnection,
}

impl Deck {
    pub fn new(
        sink: SinkController,
        source: SourceController,
        midi_out: MidiOutputConnection,
        bindings: HashMap<u8, Binding>,
    ) -> Self {
        Deck {
            sink,
            source,
            bindings,
            midi_out,
        }
    }

    pub fn flush_values_to_board(&mut self) -> Result<()> {
        for (control, binding) in self.bindings.iter() {
            match binding {
                Binding::VolumeControl(getter) => {
                    let knob = *control;
                    match getter.get_target(&mut self.sink, &mut self.source) {
                        Ok(Some(target)) => {
                            let vol = target.volume().avg();
                            let val: u8 = ((vol.0 * 127) / Volume::NORMAL.0) as u8;
                            self.midi_out.send(&[KNOB_UPDATE, knob, val])?;
                        }
                        Ok(None) => {
                            self.midi_out.send(&[KNOB_UPDATE, knob, 0])?;
                        }
                        Err(e) => {
                            println!("Could not get volume for {} : {}", knob, e);
                            return Err(e);
                        }
                    };
                }
                Binding::MuteToggle(getter) => {
                    let btn = *control;
                    match getter.get_target(&mut self.sink, &mut self.source) {
                        Ok(Some(target)) => {
                            self.midi_out
                                .send(&[BTN_DOWN, btn, target.is_muted() as u8])?;
                        }
                        Ok(None) => {
                            self.midi_out.send(&[BTN_DOWN, btn, 0])?;
                        }
                        Err(e) => {
                            println!("Could not get mute state for {} : {}", btn, e);
                            return Err(e);
                        }
                    }
                }
                Binding::DefaultSelect(getter) => {
                    let btn = *control;
                    match getter.get_target(&mut self.sink, &mut self.source) {
                        Ok(Some(target)) => {
                            let selected = match target {
                                Target::DeviceSink(device) => {
                                    Ok(self.sink.get_default_device()?.index == device.index)
                                }

                                Target::DeviceSource(device) => {
                                    Ok(self.source.get_default_device()?.index == device.index)
                                }
                                Target::AppSink(_) => {
                                    Err(anyhow!("App sinks can't be used for select bindings"))
                                }
                            }?;
                            self.midi_out
                                .send(&[BTN_DOWN, btn, if selected { 127 } else { 0 }])?;
                        }
                        Err(e) => {
                            println!("Could not get mute state for {} : {}", btn, e);
                            return Err(e);
                        }
                        Ok(None) => { /* It is valid for mappings not to have any current targets */
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub fn knob_update(&mut self, knob: u8, value: u8) -> Result<()> {
        if let Some(binding) = self.bindings.get(&knob) {
            if let VolumeControl(getter) = binding {
                match getter.get_target(&mut self.sink, &mut self.source) {
                    Ok(Some(target)) => match target {
                        Target::DeviceSink(device) => {
                            let mut vol = device.volume;
                            let new_vol = value as f32 / 127.0;
                            vol.set(
                                vol.len(),
                                Volume((new_vol * Volume::NORMAL.0 as f32) as u32),
                            );
                            self.sink.set_device_volume_by_index(device.index, &vol);
                        }
                        Target::AppSink(app) => {
                            let mut vol = app.volume;
                            let new_vol = value as f32 / 127.0;
                            vol.set(
                                vol.len(),
                                Volume((new_vol * Volume::NORMAL.0 as f32) as u32),
                            );
                            self.sink.set_sink_input_volume(app.index, &vol)?;
                        }
                        Target::DeviceSource(_) => todo!(),
                    },
                    Ok(None) => { /* It is valid for mappings not to have any current targets */ }
                    Err(e) => return Err(e),
                }
            } else {
                return Err(anyhow!("Only knobs can be bound to volume control"));
            }
        } else {
            // Ignore and zero out changes to unmapped knobs
            self.midi_out.send(&[KNOB_UPDATE, knob, 0])?;
        }
        Ok(())
    }

    pub fn btn_press(&mut self, btn: u8) -> Result<()> {
        match self.bindings.get(&btn) {
            Some(MuteToggle(getter)) => {
                match getter.get_target(&mut self.sink, &mut self.source) {
                    Ok(Some(target)) => {
                        match target {
                            Target::DeviceSink(device) => {
                                self.sink
                                    .set_device_mute_by_index(device.index, !device.mute);
                            }
                            Target::DeviceSource(device) => {
                                self.source
                                    .set_device_mute_by_index(device.index, !device.mute);
                            }
                            Target::AppSink(app) => {
                                self.sink.set_app_mute(app.index, !app.mute)?;
                            }
                        };
                    }
                    Err(e) => return Err(e),
                    Ok(None) => { /* It is valid for mappings not to have any current targets */ }
                }
            }
            Some(DefaultSelect(getter)) => {
                match getter.get_target(&mut self.sink, &mut self.source) {
                    Ok(Some(target)) => {
                        match target {
                            Target::DeviceSink(device) => {
                                let name = device
                                    .name
                                    .ok_or_else(|| anyhow!("Default device must have name"))?;
                                self.sink.set_default_device(&name)?;
                            }
                            Target::DeviceSource(device) => {
                                let name = device
                                    .name
                                    .ok_or_else(|| anyhow!("Default device must have name"))?;
                                self.source.set_default_device(&name)?;
                            }
                            Target::AppSink(_) => {
                                return Err(anyhow!("App sinks can't be used for select bindings"));
                            }
                        };
                    }
                    Err(e) => return Err(e),
                    Ok(None) => { /* It is valid for mappings not to have any current targets */ }
                }
            }
            Some(VolumeControl(_getter)) => {
                return Err(anyhow!("Buttons can not be bound to volume control"))
            }
            None => { /* unbound button, do nothing */ }
        }

        Ok(())
    }

    pub fn handle_midi_message(&mut self, message: &[u8]) -> Result<()> {
        match message {
            [KNOB_UPDATE, knob, value] => self.knob_update(*knob, *value),
            [BTN_DOWN, _btn, _value] => Ok(()),
            [BTN_UP, btn, _value] => self.btn_press(*btn),
            _ => {
                println!("Unknown message: {:?}", message);
                Ok(())
            }
        }
    }
}
