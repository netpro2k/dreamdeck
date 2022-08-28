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

type SinkGetterResult = Result<Option<Target>>;

trait Targetable {
    fn get_target(
        &self,
        sink_controller: &mut SinkController,
        source_controller: &mut SourceController,
    ) -> SinkGetterResult;
}

// impl Targetable for ApplicationInfo {
//     fn get_target(&self, sink_controller: &mut SinkController) -> SinkGetterResult {
//         let app = sink_controller.get_app_by_index(self.index)?;
//         Ok(Some(Target::AppSink(app)))
//     }
// }

struct StaticSinkDevice(DeviceInfo);
impl Targetable for StaticSinkDevice {
    fn get_target(
        &self,
        sink_controller: &mut SinkController,
        _source_controller: &mut SourceController,
    ) -> SinkGetterResult {
        let device = sink_controller.get_device_by_index(self.0.index)?;
        Ok(Some(Target::DeviceSink(device)))
    }
}

struct StaticSourceDevice(DeviceInfo);
impl Targetable for StaticSourceDevice {
    fn get_target(
        &self,
        _sink_controller: &mut SinkController,
        source_controller: &mut SourceController,
    ) -> SinkGetterResult {
        let device = source_controller.get_device_by_index(self.0.index)?;
        Ok(Some(Target::DeviceSource(device)))
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

type KnobIndex = u8;
type BtnIndex = u8;

struct Deck {
    sink: SinkController,
    source: SourceController,

    knob_map: HashMap<KnobIndex, Box<dyn Targetable>>,
    mute_map: HashMap<BtnIndex, Box<dyn Targetable>>,
    select_map: HashMap<BtnIndex, Box<dyn Targetable>>,

    midi_out: MidiOutputConnection,
}

impl Deck {
    fn flush_values_to_board(&mut self) -> Result<()> {
        for (knob, getter) in self.knob_map.iter() {
            match getter.get_target(&mut self.sink, &mut self.source) {
                Ok(Some(target)) => {
                    let vol = match target {
                        Target::DeviceSink(device) => device.volume,
                        Target::DeviceSource(device) => device.volume,
                        Target::AppSink(app) => app.volume,
                    }
                    .avg();
                    let val: u8 = ((vol.0 * 127) / Volume::NORMAL.0) as u8;
                    self.midi_out.send(&[KNOB_UPDATE, *knob, val])?;
                }
                Ok(None) => {
                    self.midi_out.send(&[KNOB_UPDATE, *knob, 0])?;
                }
                Err(e) => {
                    println!("Could not get volume for {} : {}", knob, e);
                    return Err(e);
                }
            }
        }

        for (btn, getter) in self.mute_map.iter() {
            match getter.get_target(&mut self.sink, &mut self.source) {
                Ok(Some(target)) => {
                    let muted = match target {
                        Target::DeviceSink(device) => device.mute,
                        Target::DeviceSource(device) => device.mute,
                        Target::AppSink(app) => app.mute,
                    };
                    self.midi_out
                        .send(&[BTN_DOWN, *btn, if muted { 1 } else { 0 }])?;
                }
                Ok(None) => {
                    self.midi_out.send(&[BTN_DOWN, *btn, 0])?;
                }
                Err(e) => {
                    println!("Could not get mute state for {} : {}", btn, e);
                    return Err(e);
                }
            }
        }

        for (btn, getter) in self.select_map.iter() {
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
                        .send(&[BTN_DOWN, *btn, if selected { 127 } else { 0 }])?;
                }
                Err(e) => {
                    println!("Could not get mute state for {} : {}", btn, e);
                    return Err(e);
                }
                Ok(None) => { /* It is valid for mappings not to have any current targets */ }
            }
        }

        Ok(())
    }

    fn set_sink_input_volume(&mut self, index: u32, vol: &ChannelVolumes) {
        let handler = &mut self.sink.handler;
        let op = handler.introspect.set_sink_input_volume(index, vol, None);
        handler.wait_for_operation(op).ok();
    }

    fn knob_update(&mut self, knob: u8, value: u8) -> Result<()> {
        if let Some(getter) = self.knob_map.get(&knob) {
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
                        self.set_sink_input_volume(app.index, &vol)
                    }
                    Target::DeviceSource(_) => todo!(),
                },
                Ok(None) => { /* It is valid for mappings not to have any current targets */ }
                Err(e) => return Err(e),
            }
        } else {
            // Ignore and zero out changes to unmapped knobs
            self.midi_out.send(&[KNOB_UPDATE, knob, 0])?;
        }
        Ok(())
    }

    fn btn_press(&mut self, btn: u8) -> Result<()> {
        if let Some(getter) = self.mute_map.get(&btn) {
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
                Err(e) => {
                    return Err(anyhow!("Could not get mute state for {} : {}", btn, e));
                }
                Ok(None) => { /* It is valid for mappings not to have any current targets */ }
            }
        }
        if let Some(getter) = self.select_map.get(&btn) {
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
                Err(e) => {
                    println!("Could not get mute state for {} : {}", btn, e);
                    todo!();
                    // return Err(e);
                }
                Ok(None) => { /* It is valid for mappings not to have any current targets */ }
            }
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

    let mut knob_mappings: HashMap<KnobIndex, Box<dyn Targetable>> = HashMap::new();
    let mut mute_mappings: HashMap<BtnIndex, Box<dyn Targetable>> = HashMap::new();
    let mut select_mappings: HashMap<BtnIndex, Box<dyn Targetable>> = HashMap::new();

    knob_mappings.insert(0xB, Box::new(StaticSinkDevice(speakers.clone())));
    knob_mappings.insert(0xC, Box::new(StaticSinkDevice(headphones.clone())));
    knob_mappings.insert(
        0xD,
        Box::new(FirstValidTarget(vec![
            Box::new(AlwaysNone {}),
            // Box::new(AlwaysError {}),
            Box::new(PropertyMatchSink(properties::APPLICATION_NAME, "Firefox")),
        ])),
    );

    select_mappings.insert(0x20, Box::new(StaticSinkDevice(speakers.clone())));
    select_mappings.insert(0x21, Box::new(StaticSinkDevice(headphones.clone())));
    mute_mappings.insert(0x22, Box::new(StaticSourceDevice(mic)));

    mute_mappings.insert(0x28, Box::new(StaticSinkDevice(speakers.clone())));
    mute_mappings.insert(0x29, Box::new(StaticSinkDevice(headphones.clone())));
    mute_mappings.insert(
        0x30,
        Box::new(FirstValidTarget(vec![
            Box::new(AlwaysNone {}),
            // Box::new(AlwaysError {}),
            Box::new(PropertyMatchSink(properties::APPLICATION_NAME, "Firefox")),
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

        knob_map: knob_mappings,
        mute_map: mute_mappings,
        select_map: select_mappings,

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
