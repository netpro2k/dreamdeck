mod binding;
mod deck;
mod target;

use core::time;

use std::{collections::HashMap, thread};

use pulsectl::controllers::DeviceControl;
use pulsectl::controllers::SinkController;
use pulsectl::controllers::SourceController;
use std::sync::mpsc::channel;

use midir::{MidiInput, MidiOutput};

use anyhow::Result;
use binding::Binding;

use deck::Deck;

use target::*;

const SPEAKER_SINK : &str = "alsa_output.usb-Apple__Inc._USB-C_to_3.5mm_Headphone_Jack_Adapter_DWH931705N1JKLTAL-00.analog-stereo";
const HEADPHONE_SINK :&str = "alsa_output.usb-Apple__Inc._USB-C_to_3.5mm_Headphone_Jack_Adapter_DWH9317032QJKLTAR-00.analog-stereo";
const MIC_SOURCE :&str ="alsa_input.usb-Apple__Inc._USB-C_to_3.5mm_Headphone_Jack_Adapter_DWH9317032QJKLTAR-00.mono-fallback";
// const LINEIN_SOURCE: &str = "alsa_input.pci-0000_00_1f.3.analog-stereo";

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
        Binding::volume(FirstValidTarget::new(vec![
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
        Binding::mute(FirstValidTarget::new(vec![
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

    let mut deck = Deck::new(sink_controller, source_controller, midi_out, b);

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
                    deck.handle_midi_message(&midi_msg)?;
                }
            },
            Err(_) => break Err("Hung up".into()),
        }
    }
}
