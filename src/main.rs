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

const SPEAKER_SINK : &str = "alsa_output.usb-Apple__Inc._USB-C_to_3.5mm_Headphone_Jack_Adapter_DWH931705N1JKLTAL-00.analog-stereo";
const HEADPHONE_SINK :&str = "alsa_output.usb-Apple__Inc._USB-C_to_3.5mm_Headphone_Jack_Adapter_DWH9317032QJKLTAR-00.analog-stereo";
// const MIC_SOURCE :&str ="alsa_input.usb-Apple__Inc._USB-C_to_3.5mm_Headphone_Jack_Adapter_DWH9317032QJKLTAR-00.mono-fallback";
// const LINEIN_SOURCE: &str = "alsa_input.pci-0000_00_1f.3.analog-stereo";

const KNOB_UPDATE: u8 = 0xBA;
// const BTN_UPDATE: u8 = 0x8A;

#[derive(Debug)]
enum SinkTarget {
    DeviceSink(u32),
    AppSink(u32),
}

trait SinkGetter {
    fn get_target(&self, controller: &mut SinkController) -> Option<SinkTarget>;
}

impl SinkGetter for ApplicationInfo {
    fn get_target(&self, _controller: &mut SinkController) -> Option<SinkTarget> {
        Some(SinkTarget::AppSink(self.index))
    }
}

impl SinkGetter for DeviceInfo {
    fn get_target(&self, _controller: &mut SinkController) -> Option<SinkTarget> {
        Some(SinkTarget::DeviceSink(self.index))
    }
}

struct PropertyMatchSink<'a> {
    prop: &'a str,
    value: &'a str,
}

impl PropertyMatchSink<'_> {
    fn get_info(&self, controller: &mut SinkController) -> Option<ApplicationInfo> {
        let apps = controller.list_applications().expect("Failed to get apps");
        for app in apps {
            if let Some(true) = app.proplist.get_str(self.prop).map(|v| self.value == v) {
                return Some(app);
            }
        }
        None
    }
}

impl SinkGetter for PropertyMatchSink<'_> {
    fn get_target(&self, controller: &mut SinkController) -> Option<SinkTarget> {
        self.get_info(controller)
            .as_ref()
            .map(|app| SinkTarget::AppSink(app.index))
    }
}

type KnobIndex = u8;

struct Deck {
    sink_controller: SinkController,
    // midi_in: &'a MidiInput,
    // midi_out: &'a RtMidiOut,
    knob_mappings: HashMap<KnobIndex, Box<dyn SinkGetter>>,
    midi_out: MidiOutputConnection,
}

impl Deck {
    fn flush_values_to_board(&mut self) {
        for (knob, getter) in self.knob_mappings.iter() {
            match getter.get_target(&mut self.sink_controller) {
                Some(target) => {
                    let vol = match target {
                        SinkTarget::DeviceSink(index) => self
                            .sink_controller
                            .get_device_by_index(index)
                            .ok()
                            .map(|d| d.volume),

                        SinkTarget::AppSink(index) => self
                            .sink_controller
                            .get_app_by_index(index)
                            .ok()
                            .map(|d| d.volume),
                    };
                    if let Some(vol) = vol {
                        let vol = vol.avg();
                        let val: u8 = ((vol.0 * 127) / Volume::NORMAL.0) as u8;
                        println!("Set {} to {}", knob, vol);
                        self.midi_out.send(&[KNOB_UPDATE, *knob, val]);
                    } else {
                        println!("Could not get volume for {} {:?}", knob, target);
                    }
                }
                None => {
                    println!("Could not get volume for {}", knob);
                }
            }
        }
    }

    fn set_sink_input_volume(&mut self, index: u32, vol: &ChannelVolumes) {
        let handler = &mut self.sink_controller.handler;
        let op = handler.introspect.set_sink_input_volume(index, vol, None);
        handler.wait_for_operation(op).ok();
    }

    fn knob_update(&mut self, knob: u8, value: u8) {
        if let Some(getter) = self.knob_mappings.get(&knob) {
            if let Some(target) = getter.get_target(&mut self.sink_controller) {
                match target {
                    SinkTarget::DeviceSink(index) => {
                        if let Ok(device) = self.sink_controller.get_device_by_index(index) {
                            let mut vol = device.volume;
                            let new_vol = value as f32 / 127.0;
                            vol.set(
                                vol.len(),
                                Volume((new_vol * Volume::NORMAL.0 as f32) as u32),
                            );

                            println!("Knob update {} = {} {}", knob, value, new_vol);

                            self.sink_controller.set_device_volume_by_index(index, &vol);

                            // self.set_sink_volume(device.index, &vol)
                        }
                    }
                    SinkTarget::AppSink(index) => {
                        if let Ok(app) = self.sink_controller.get_app_by_index(index) {
                            let mut vol = app.volume;
                            let new_vol = value as f32 / 127.0;
                            vol.set(
                                vol.len(),
                                Volume((new_vol * Volume::NORMAL.0 as f32) as u32),
                            );

                            println!("Knob update {} = {} {}, {}", knob, value, new_vol, index);
                            self.set_sink_input_volume(index, &vol)
                        }
                    }
                }
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

enum Msg {
    SyncBoard,
    MidiUpdate([u8; 3]),
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut sink_controller = SinkController::create()?;

    // let op = self
    //     .handler
    //     .introspect
    //     .set_sink_volume_by_index(index, &volumes, None);
    // self.handler.wait_for_operation(op).ok();

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
        Box::new(PropertyMatchSink {
            prop: properties::APPLICATION_NAME,
            value: "Firefox",
        }),
    );

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
            thread::sleep(time::Duration::from_millis(10000))
        }
    });

    loop {
        let msg = rx.recv();
        match msg {
            Ok(msg) => match msg {
                Msg::SyncBoard => {
                    // println!("Sync from pulse");
                    deck.flush_values_to_board();
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
