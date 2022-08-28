use core::time;
use pulse::{
    proplist::properties,
    volume::{ChannelVolumes, Volume},
};
use std::{collections::HashMap, thread};

use pulsectl::controllers::types::*;
use pulsectl::controllers::AppControl;
use pulsectl::controllers::DeviceControl;
use pulsectl::controllers::SinkController;
use std::sync::mpsc::channel;

use midir::{Ignore, MidiInput, MidiOutput, MidiOutputConnection};

use anyhow::{anyhow, Result};

const SPEAKER_SINK : &str = "alsa_output.usb-Apple__Inc._USB-C_to_3.5mm_Headphone_Jack_Adapter_DWH931705N1JKLTAL-00.analog-stereo";
const HEADPHONE_SINK :&str = "alsa_output.usb-Apple__Inc._USB-C_to_3.5mm_Headphone_Jack_Adapter_DWH9317032QJKLTAR-00.analog-stereo";
// const MIC_SOURCE :&str ="alsa_input.usb-Apple__Inc._USB-C_to_3.5mm_Headphone_Jack_Adapter_DWH9317032QJKLTAR-00.mono-fallback";
// const LINEIN_SOURCE: &str = "alsa_input.pci-0000_00_1f.3.analog-stereo";

const KNOB_UPDATE: u8 = 0xBA;
// const BTN_UPDATE: u8 = 0x8A;

enum SinkTarget {
    DeviceSink(DeviceInfo),
    AppSink(ApplicationInfo),
}

type SinkGetterResult = Result<Option<SinkTarget>>;

trait SinkGetter {
    fn get_target(&self, sink_controller: &mut SinkController) -> SinkGetterResult;
}

impl SinkGetter for ApplicationInfo {
    fn get_target(&self, sink_controller: &mut SinkController) -> SinkGetterResult {
        let app = sink_controller.get_app_by_index(self.index)?;
        Ok(Some(SinkTarget::AppSink(app)))
    }
}

impl SinkGetter for DeviceInfo {
    fn get_target(&self, sink_controller: &mut SinkController) -> SinkGetterResult {
        let device = sink_controller.get_device_by_index(self.index)?;
        Ok(Some(SinkTarget::DeviceSink(device)))
    }
}

struct PropertyMatchSink<'a> {
    prop: &'a str,
    value: &'a str,
}

impl PropertyMatchSink<'_> {
    fn find_app(&self, sink_controller: &mut SinkController) -> Result<Option<ApplicationInfo>> {
        let apps = sink_controller.list_applications()?;
        Ok(apps.into_iter().find(|app| {
            app.proplist
                .get_str(self.prop)
                .filter(|v| self.value == v)
                .is_some()
        }))
    }
}

impl SinkGetter for PropertyMatchSink<'_> {
    fn get_target(&self, sink_controller: &mut SinkController) -> SinkGetterResult {
        let app = self.find_app(sink_controller)?;
        Ok(app.map(|app| SinkTarget::AppSink(app)))
    }
}

struct FirstValidTarget {
    getters: Vec<Box<dyn SinkGetter>>,
}

impl SinkGetter for FirstValidTarget {
    fn get_target(&self, sink_controller: &mut SinkController) -> SinkGetterResult {
        // We want to get the first non-None target but still propagate errors up
        let first_valid = self
            .getters
            .iter()
            .map(|g| g.get_target(sink_controller))
            .filter(|g| g.is_err() || g.as_ref().unwrap().is_some())
            .next();
        match first_valid {
            Some(r) => r,
            None => Ok(None),
        }
    }
}

type KnobIndex = u8;

struct Deck {
    sink_controller: SinkController,
    knob_mappings: HashMap<KnobIndex, Box<dyn SinkGetter>>,
    midi_out: MidiOutputConnection,
}

impl Deck {
    fn flush_values_to_board(&mut self) -> Result<()> {
        for (knob, getter) in self.knob_mappings.iter() {
            match getter.get_target(&mut self.sink_controller) {
                Ok(Some(target)) => {
                    let vol = match target {
                        SinkTarget::DeviceSink(device) => device.volume,
                        SinkTarget::AppSink(app) => app.volume,
                    };
                    let vol = vol.avg();
                    let val: u8 = ((vol.0 * 127) / Volume::NORMAL.0) as u8;
                    self.midi_out.send(&[KNOB_UPDATE, *knob, val])?;
                }
                Err(e) => {
                    println!("Could not get volume for {} : {}", knob, e);
                    return Err(e);
                }
                Ok(None) => { /* It is valid for mappings not to have any current targets */ }
            }
        }
        Ok(())
    }

    fn set_sink_input_volume(&mut self, index: u32, vol: &ChannelVolumes) {
        let handler = &mut self.sink_controller.handler;
        let op = handler.introspect.set_sink_input_volume(index, vol, None);
        handler.wait_for_operation(op).ok();
    }

    fn knob_update(&mut self, knob: u8, value: u8) {
        if let Some(getter) = self.knob_mappings.get(&knob) {
            match getter.get_target(&mut self.sink_controller) {
                Ok(Some(target)) => match target {
                    SinkTarget::DeviceSink(device) => {
                        let mut vol = device.volume;
                        let new_vol = value as f32 / 127.0;
                        vol.set(
                            vol.len(),
                            Volume((new_vol * Volume::NORMAL.0 as f32) as u32),
                        );

                        println!("Knob update {} = {} {}", knob, value, new_vol);

                        self.sink_controller
                            .set_device_volume_by_index(device.index, &vol);
                    }
                    SinkTarget::AppSink(app) => {
                        let mut vol = app.volume;
                        let new_vol = value as f32 / 127.0;
                        vol.set(
                            vol.len(),
                            Volume((new_vol * Volume::NORMAL.0 as f32) as u32),
                        );

                        println!("Knob update {} = {} {}", knob, value, new_vol);
                        self.set_sink_input_volume(app.index, &vol)
                    }
                },
                Err(_) => todo!(),
                Ok(None) => { /* It is valid for mappings not to have any current targets */ }
            }
        }
    }

    fn midi_message(&mut self, message: &[u8]) {
        match message {
            [KNOB_UPDATE, knob, value] => self.knob_update(*knob, *value),
            _ => println!("Unknown message: {:?}", message),
        };
    }
}

struct AlwaysNone {}
impl SinkGetter for AlwaysNone {
    fn get_target(&self, _sink_controller: &mut SinkController) -> SinkGetterResult {
        return Ok(None);
    }
}

struct AlwaysError {}
impl SinkGetter for AlwaysError {
    fn get_target(&self, _sink_controller: &mut SinkController) -> SinkGetterResult {
        return Err(anyhow!("AlwaysError always errors"));
    }
}

enum Msg {
    SyncBoard,
    MidiUpdate([u8; 3]),
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut sink_controller = SinkController::create()?;

    let mut midi_in = MidiInput::new("DreamDeak in")?;
    midi_in.ignore(Ignore::None);
    let in_ports = midi_in.ports();

    let midi_out = MidiOutput::new("DreamDeck out")?;
    let out_ports = midi_out.ports();

    println!("\nOpening connection");
    let in_port_name = midi_in.port_name(&in_ports[1])?;
    println!("{}", in_port_name);

    let mut mappings: HashMap<KnobIndex, Box<dyn SinkGetter>> = HashMap::new();
    let speakers = sink_controller.get_device_by_name(SPEAKER_SINK)?;
    let headphones = sink_controller.get_device_by_name(HEADPHONE_SINK)?;
    mappings.insert(11, Box::new(speakers));
    mappings.insert(12, Box::new(headphones));
    mappings.insert(
        13,
        Box::new(FirstValidTarget {
            getters: vec![
                Box::new(AlwaysNone {}),
                // Box::new(AlwaysError {}),
                Box::new(PropertyMatchSink {
                    prop: properties::APPLICATION_NAME,
                    value: "Firefox",
                }),
            ],
        }),
    );
    // mappings.insert(14, Box::new(AlwaysError {}));

    let (tx, rx) = channel();
    let midi_tx = tx.clone();
    let _midi_in = midi_in.connect(
        &in_ports[1],
        "DreamDeck read",
        move |stamp, message, _| {
            println!("{}: {:?} (len = {})", stamp, message, message.len());

            midi_tx
                .send(Msg::MidiUpdate([message[0], message[1], message[2]]))
                .expect("failed to send midi message to main thread");
        },
        (),
    )?;
    let midi_out = midi_out.connect(&out_ports[1], "DreamDeck write")?;

    let mut deck = Deck {
        sink_controller,

        // midi_in: &midi_in,
        // midi_out: &output,
        knob_mappings: mappings,
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
                    println!("{:?}", midi_msg);
                    deck.midi_message(&midi_msg);
                }
            },
            Err(_) => break Err("Hung up".into()),
        }
    }
}
