use std::collections::HashMap;

use anyhow::{anyhow, Result};
use midir::MidiOutputConnection;
use pulse::volume::Volume;
use pulsectl::controllers::{SinkController, SourceController};

use crate::{binding::Binding, binding::Binding::*};

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

    pub fn clear(&mut self) -> Result<()> {
        for i in 11..=18 {
            self.midi_out.send(&[KNOB_UPDATE, i, 0])?;
        }
        for i in 24..=47 {
            self.midi_out.send(&[BTN_DOWN, i, 0])?;
        }
        Ok(())
    }

    pub fn flush_values_to_board(&mut self) -> Result<()> {
        for (&control, binding) in self.bindings.iter() {
            match binding {
                Binding::VolumeControl(target) => {
                    if let Some(vol) = target.volume(&mut self.sink, &mut self.source)? {
                        let val: u8 = ((vol.0 * 127) / Volume::NORMAL.0) as u8;
                        self.midi_out
                            .send(&[KNOB_UPDATE, control, val.clamp(0, 127)])?;
                    } else {
                        self.midi_out.send(&[KNOB_UPDATE, control, 0])?;
                    }
                }
                Binding::MuteToggle(target) => {
                    if let Some(is_muted) = target.muted(&mut self.sink, &mut self.source)? {
                        self.midi_out.send(&[BTN_DOWN, control, is_muted.into()])?;
                    } else {
                        self.midi_out.send(&[BTN_DOWN, control, 0])?;
                    }
                }
                Binding::DefaultSelect(target) => {
                    let is_selected = target
                        .selected(&mut self.sink, &mut self.source)?
                        .unwrap_or_default();
                    self.midi_out
                        .send(&[BTN_DOWN, control, is_selected.into()])?;
                }
            }
        }

        Ok(())
    }

    pub fn knob_update(&mut self, knob: u8, value: u8) -> Result<()> {
        if let Some(binding) = self.bindings.get(&knob) {
            if let VolumeControl(target) = binding {
                target.set_volume(&mut self.sink, &mut self.source, value as f32 / 127.0)?;
            } else {
                return Err(anyhow!("Only knobs can be bound to volume control"));
            }
        } else {
            // Ignore and zero out changes to unmapped knobs
            self.midi_out.send(&[KNOB_UPDATE, knob, 0])?
        }
        Ok(())
    }

    pub fn btn_press(&mut self, btn: u8) -> Result<()> {
        match self.bindings.get(&btn) {
            Some(MuteToggle(target)) => {
                target.toggle_muted(&mut self.sink, &mut self.source)?;
            }
            Some(DefaultSelect(target)) => {
                target.set_as_selected(&mut self.sink, &mut self.source)?;
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
            [BTN_DOWN, btn, _value] => self.btn_press(*btn),
            [BTN_UP, _btn, _value] => Ok(()),
            _ => {
                println!("Unknown message: {:?}", message);
                Ok(())
            }
        }
    }
}
