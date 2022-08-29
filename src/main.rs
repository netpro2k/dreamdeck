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

fn make_config(
    sink_controller: &mut SinkController,
    source_controller: &mut SourceController,
) -> Result<HashMap<u8, Binding>> {
    let speakers = sink_controller.get_device_by_name(SPEAKER_SINK)?;
    let headphones = sink_controller.get_device_by_name(HEADPHONE_SINK)?;
    let mic = source_controller.get_device_by_name(MIC_SOURCE)?;

    let mut bindings = HashMap::from([
        (11, Binding::volume(Sink::from(&speakers))),
        (12, Binding::volume(Sink::from(&headphones))),
        (
            13,
            Binding::volume(FirstValidTarget::new(vec![
                PropertyMatchSink::app_name("WEBRTC VoiceEngine"), // Discord
                PropertyMatchSink::app_name("ZOOM VoiceEngine"),
            ])),
        ),
        (
            14,
            Binding::volume(FirstValidTarget::new(vec![
                // TODO control multiple targets with 1 knob/button
                //             sink_getter_all_by_property("application.name", "FINAL FANTASY XIV"),
                PropertyMatchSink::app_name("ALSA plug-in [wine64-preloader]"),
                PropertyMatchSink::app_name("Among Us.exe"),
                PropertyMatchSink::app_name("Spel2.exe"), // Spelunky 2
                PropertyMatchSink::app_name("FMOD Ex App"),
                PropertyMatchSink::app_name("Risk of Rain 2.exe"),
                PropertyMatchSink::process_binary("DyingLightGame"),
                // Generic games running under wine
                PropertyMatchSink::app_name("wine-preloader"),
                PropertyMatchSink::app_name("wine64-preloader"),
                PropertyMatchSink::process_binary("wine-preloader"),
                PropertyMatchSink::process_binary("wine64-preloader"),
                // Steam Streaming
                PropertyMatchSink::process_binary("streaming_client"),
            ])),
        ),
        (
            15,
            Binding::volume(FirstValidTarget::new(vec![
                PropertyMatchSink::app_name("Google Play Music Desktop Player"),
                PropertyMatchSink::app_name("mpv Media Player"),
            ])),
        ),
        (
            16,
            Binding::volume(*PropertyMatchSink::media_name("Loopback of Onboard Audio")),
        ),
        (
            17,
            Binding::volume(*PropertyMatchSink::app_name("Moonlight")),
        ),
        (32, Binding::select(Sink::from(&speakers))),
        (33, Binding::select(Sink::from(&headphones))),
        (34, Binding::mute(Source::from(&mic))),
    ]);

    // Bind the bottom row of buttons to mute the thing the knob in that column controls the volume of
    for i in 0..=6 {
        println!("Binding {} to mute {}", 40 + i, 11 + i);
        bindings.insert(40 + i, bindings.get(&(11 + i)).unwrap().to_mute());
    }

    Ok(bindings)
}

enum Msg {
    SyncBoard,
    MidiUpdate([u8; 3]),
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let midi_out = MidiOutput::new("DreamDeck out")?;
    let port = &midi_out.ports()[1];
    let midi_out = midi_out.connect(port, "DreamDeck write")?;

    let midi_in = MidiInput::new("DreamDeak in")?;
    // midi_in.ignore(Ignore::None);
    let in_ports = midi_in.ports();

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

    let _poll_thread = thread::spawn(move || {
        // thread code
        loop {
            tx.send(Msg::SyncBoard)
                .expect("failed to send sync message to main thread");
            thread::sleep(time::Duration::from_millis(100))
        }
    });

    let mut sink_controller = SinkController::create()?;
    let mut source_controller = SourceController::create()?;
    let bindings = make_config(&mut sink_controller, &mut source_controller)?;
    let mut deck = Deck::new(sink_controller, source_controller, midi_out, bindings);

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
