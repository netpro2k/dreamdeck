use core::time;
use pulse::{
    proplist::properties,
    volume::{ChannelVolumes, Volume},
};
use std::{collections::HashMap, thread};

use pulsectl::controllers::AppControl;
use pulsectl::controllers::DeviceControl;
use pulsectl::controllers::SinkController;
use pulsectl::controllers::{types::*, SourceController};
use std::sync::mpsc::channel;

use midir::{MidiInput, MidiOutput, MidiOutputConnection};

use anyhow::{anyhow, Result};
use Binding::*;

const SPEAKER_SINK : &str = "alsa_output.usb-Apple__Inc._USB-C_to_3.5mm_Headphone_Jack_Adapter_DWH931705N1JKLTAL-00.analog-stereo";
const HEADPHONE_SINK :&str = "alsa_output.usb-Apple__Inc._USB-C_to_3.5mm_Headphone_Jack_Adapter_DWH9317032QJKLTAR-00.analog-stereo";
const MIC_SOURCE :&str ="alsa_input.usb-Apple__Inc._USB-C_to_3.5mm_Headphone_Jack_Adapter_DWH9317032QJKLTAR-00.mono-fallback";
// const LINEIN_SOURCE: &str = "alsa_input.pci-0000_00_1f.3.analog-stereo";

const KNOB_UPDATE: u8 = 0xBA;
const BTN_DOWN: u8 = 0x9A;
const BTN_UP: u8 = 0x8A;

enum Target {
    DeviceSink(DeviceInfo),
    AppSink(ApplicationInfo),
    DeviceSource(DeviceInfo),
    // AppSource(ApplicationInfo),
}
impl Target {
    fn is_muted(&self) -> bool {
        match self {
            Target::DeviceSink(device) => device.mute,
            Target::DeviceSource(device) => device.mute,
            Target::AppSink(app) => app.mute,
        }
    }
    fn volume(&self) -> ChannelVolumes {
        match self {
            Target::DeviceSink(device) => device.volume,
            Target::DeviceSource(device) => device.volume,
            Target::AppSink(app) => app.volume,
        }
    }
}

type SinkGetterResult = Result<Option<Target>>;

trait Targetable {
    fn get_target(
        &self,
        sink_controller: &mut SinkController,
        source_controller: &mut SourceController,
    ) -> SinkGetterResult;
}

enum Binding {
    VolumeControl(Box<dyn Targetable>),
    MuteToggle(Box<dyn Targetable>),
    DefaultSelect(Box<dyn Targetable>),
}
impl Binding {
    pub fn volume<T: Targetable + 'static>(t: T) -> Binding {
        VolumeControl(Box::new(t))
    }
}
impl Binding {
    pub fn mute<T: Targetable + 'static>(t: T) -> Binding {
        MuteToggle(Box::new(t))
    }
}
impl Binding {
    pub fn select<T: Targetable + 'static>(t: T) -> Binding {
        Self::DefaultSelect(Box::new(t))
    }
}

// impl Targetable for ApplicationInfo {
//     fn get_target(&self, sink_controller: &mut SinkController) -> SinkGetterResult {
//         let app = sink_controller.get_app_by_index(self.index)?;
//         Ok(Some(Target::AppSink(app)))
//     }
// }

struct Sink(u32);
impl Targetable for Sink {
    fn get_target(
        &self,
        sink_controller: &mut SinkController,
        _source_controller: &mut SourceController,
    ) -> SinkGetterResult {
        let device = sink_controller.get_device_by_index(self.0)?;
        Ok(Some(Target::DeviceSink(device)))
    }
}
impl From<&DeviceInfo> for Sink {
    fn from(d: &DeviceInfo) -> Self {
        Self(d.index)
    }
}

struct Source(u32);
impl Targetable for Source {
    fn get_target(
        &self,
        _sink_controller: &mut SinkController,
        source_controller: &mut SourceController,
    ) -> SinkGetterResult {
        let device = source_controller.get_device_by_index(self.0)?;
        Ok(Some(Target::DeviceSource(device)))
    }
}
impl From<&DeviceInfo> for Source {
    fn from(d: &DeviceInfo) -> Self {
        Self(d.index)
    }
}

struct PropertyMatchSink<'a>(&'a str, &'a str);
impl PropertyMatchSink<'_> {
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
        Box::new(PropertyMatchSink(properties::APPLICATION_NAME, name))
    }
}
impl Targetable for PropertyMatchSink<'_> {
    fn get_target(
        &self,
        sink_controller: &mut SinkController,
        _source_controller: &mut SourceController,
    ) -> SinkGetterResult {
        let app = self.find_app(sink_controller)?;
        Ok(app.map(|app| Target::AppSink(app)))
    }
}

struct FirstValidTarget(Vec<Box<dyn Targetable>>);
impl Targetable for FirstValidTarget {
    fn get_target(
        &self,
        sink_controller: &mut SinkController,
        source_controller: &mut SourceController,
    ) -> SinkGetterResult {
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

trait SinkControllerExt {
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

struct Deck {
    sink: SinkController,
    source: SourceController,

    bindings: HashMap<u8, Binding>,

    midi_out: MidiOutputConnection,
}

impl Deck {
    fn flush_values_to_board(&mut self) -> Result<()> {
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

    fn knob_update(&mut self, knob: u8, value: u8) -> Result<()> {
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

    fn btn_press(&mut self, btn: u8) -> Result<()> {
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

    fn midi_message(&mut self, message: &[u8]) -> Result<()> {
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

struct AlwaysNone {}
impl Targetable for AlwaysNone {
    fn get_target(
        &self,
        _sink_controller: &mut SinkController,
        _source_controller: &mut SourceController,
    ) -> SinkGetterResult {
        return Ok(None);
    }
}

struct AlwaysError {}
impl Targetable for AlwaysError {
    fn get_target(
        &self,
        _sink_controller: &mut SinkController,
        _source_controller: &mut SourceController,
    ) -> SinkGetterResult {
        return Err(anyhow!("AlwaysError always errors"));
    }
}

enum Msg {
    SyncBoard,
    MidiUpdate([u8; 3]),
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut sink_controller = SinkController::create()?;
    let mut source_controller = SourceController::create()?;

    let midi_in = MidiInput::new("DreamDeak in")?;
    // midi_in.ignore(Ignore::None);
    let in_ports = midi_in.ports();

    let midi_out = MidiOutput::new("DreamDeck out")?;
    let out_ports = midi_out.ports();

    println!("Opening connection");
    let in_port_name = midi_in.port_name(&in_ports[1])?;
    println!("{}", in_port_name);

    let speakers = sink_controller.get_device_by_name(SPEAKER_SINK)?;
    let headphones = sink_controller.get_device_by_name(HEADPHONE_SINK)?;
    let mic = source_controller.get_device_by_name(MIC_SOURCE)?;

    let mut b = HashMap::new();
    b.insert(11, Binding::volume(Sink::from(&speakers)));
    b.insert(12, Binding::volume(Sink::from(&headphones)));
    b.insert(
        13,
        Binding::volume(FirstValidTarget(vec![
            Box::new(AlwaysNone {}),
            // Box::new(AlwaysError {}),
            PropertyMatchSink::app_name("Firefox"),
        ])),
    );
    b.insert(32, Binding::select(Sink::from(&speakers)));
    b.insert(33, Binding::select(Sink::from(&headphones)));
    b.insert(34, Binding::mute(Source::from(&mic)));

    b.insert(40, Binding::mute(Sink::from(&speakers)));
    b.insert(41, Binding::mute(Sink::from(&headphones)));
    b.insert(
        42,
        Binding::mute(FirstValidTarget(vec![
            Box::new(AlwaysNone {}),
            // Box::new(AlwaysError {}),
            PropertyMatchSink::app_name("Firefox"),
        ])),
    );

    // mappings.insert(14, Box::new(AlwaysError {}));

    let (tx, rx) = channel();
    let midi_tx = tx.clone();
    let _midi_in = midi_in.connect(
        &in_ports[1],
        "DreamDeck read",
        move |_stamp, message, _| {
            println!("{}: {:?} (len = {})", _stamp, message, message.len());

            midi_tx
                .send(Msg::MidiUpdate([message[0], message[1], message[2]]))
                .expect("failed to send midi message to main thread");
        },
        (),
    )?;
    let midi_out = midi_out.connect(&out_ports[1], "DreamDeck write")?;

    let mut deck = Deck {
        sink: sink_controller,
        source: source_controller,

        bindings: b,

        midi_out,
    };

    let _poll_thread = thread::spawn(move || {
        // thread code
        loop {
            tx.send(Msg::SyncBoard)
                .expect("failed to send sync message to main thread");
            thread::sleep(time::Duration::from_millis(100))
        }
    });

    loop {
        let msg = rx.recv();
        match msg {
            Ok(msg) => match msg {
                Msg::SyncBoard => {
                    // println!("Sync from pulse");
                    deck.flush_values_to_board()?;
                }
                Msg::MidiUpdate(midi_msg) => {
                    // println!("{:?}", midi_msg);
                    deck.midi_message(&midi_msg)?;
                }
            },
            Err(_) => break Err("Hung up".into()),
        }
    }
}
