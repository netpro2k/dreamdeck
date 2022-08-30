mod binding;
mod deck;
mod target;

use core::time;

use std::{collections::HashMap, thread};

use pulse::proplist::properties::APPLICATION_NAME;
use pulse::proplist::properties::APPLICATION_PROCESS_BINARY;
use pulse::proplist::properties::MEDIA_NAME;
use pulsectl::controllers::DeviceControl;
use pulsectl::controllers::SinkController;
use pulsectl::controllers::SourceController;
use std::sync::mpsc::channel;

use midir::{MidiInput, MidiOutput};

use anyhow::Result;
use binding::Binding;

use deck::Deck;

use target::Target::*;

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

    // Layer B
    //
    // (11) (12) (13) (14) (15) (16) (17) (18)  Knob Turn
    // [24] [25] [26] [27] [28] [29] [30] [31]  Knob Press
    //
    // [32] [33] [34] [35] [36] [37] [38] [39]  Buttons
    // [40] [41] [42] [43] [44] [45] [46] [47]

    let mut bindings = HashMap::from([
        (11, Binding::volume(StaticSink(speakers.index))),
        (12, Binding::volume(StaticSink(headphones.index))),
        (
            13,
            Binding::volume(Any(vec![
                SinkWithProperty(APPLICATION_NAME, "WEBRTC VoiceEngine"), // Discord
                SinkWithProperty(APPLICATION_NAME, "ZOOM VoiceEngine"),
            ])),
        ),
        (
            14,
            Binding::volume(Any(vec![
                // TODO control multiple targets with 1 knob/button
                //             sink_getter_all_by_property("application.name", "FINAL FANTASY XIV"),
                SinkWithProperty(APPLICATION_NAME, "ALSA plug-in [wine64-preloader]"),
                SinkWithProperty(APPLICATION_NAME, "Among Us.exe"),
                SinkWithProperty(APPLICATION_NAME, "Spel2.exe"), // Spelunky 2
                SinkWithProperty(APPLICATION_NAME, "FMOD Ex App"),
                SinkWithProperty(APPLICATION_NAME, "Risk of Rain 2.exe"),
                SinkWithProperty(APPLICATION_PROCESS_BINARY, "DyingLightGame"),
                // Generic games running under wine
                SinkWithProperty(APPLICATION_NAME, "wine-preloader"),
                SinkWithProperty(APPLICATION_NAME, "wine64-preloader"),
                SinkWithProperty(APPLICATION_PROCESS_BINARY, "wine-preloader"),
                SinkWithProperty(APPLICATION_PROCESS_BINARY, "wine64-preloader"),
                // Steam Streaming
                SinkWithProperty(APPLICATION_PROCESS_BINARY, "streaming_client"),
            ])),
        ),
        (
            15,
            Binding::volume(Any(vec![
                SinkWithProperty(APPLICATION_NAME, "Google Play Music Desktop Player"),
                SinkWithProperty(APPLICATION_NAME, "mpv Media Player"),
            ])),
        ),
        (
            16,
            Binding::volume(SinkWithProperty(MEDIA_NAME, "Loopback of Onboard Audio")),
        ),
        (
            17,
            Binding::volume(SinkWithProperty(APPLICATION_NAME, "Moonlight")),
        ),
        (32, Binding::select(StaticSink(speakers.index))),
        (33, Binding::select(StaticSink(headphones.index))),
        (34, Binding::mute(StaticSource(mic.index))),
    ]);

    // Bind the bottom row of buttons to mute the thing the knob in that column controls the volume of
    for i in 0..=6 {
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
