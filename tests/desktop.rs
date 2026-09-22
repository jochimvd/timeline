use std::{
    io::{Read, Write},
    os::unix::net::UnixListener,
    sync::mpsc,
    thread,
};
use timeline::desktop::Hyprland;

#[test]
fn focus_events_preserve_rapid_switches_commas_and_no_focus() {
    let directory = tempfile::tempdir().unwrap();
    let commands = UnixListener::bind(directory.path().join(".socket.sock")).unwrap();
    let events = UnixListener::bind(directory.path().join(".socket2.sock")).unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut event_stream, _) = events.accept().unwrap();
        let (mut command_stream, _) = commands.accept().unwrap();
        let mut command = [0; 14];
        command_stream.read_exact(&mut command).unwrap();
        assert_eq!(&command, b"j/activewindow");
        command_stream
            .write_all(br#"{"address":"0xabc","class":"editor","title":"original"}"#)
            .unwrap();
        drop(command_stream);
        event_stream.write_all(b"activewindow>>chat,one,two\nactivewindowv2>>def\nwindowtitlev2>>def,new, title\nactivewindow>>,\nactivewindowv2>>\n").unwrap();
        ready_tx.send(()).unwrap();
        done_rx.recv().unwrap();
    });
    let mut source = Hyprland::connect(directory.path()).unwrap();
    ready_rx.recv().unwrap();
    let changes = source.drain().unwrap();
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[0].as_ref().unwrap().title, "one,two");
    assert_eq!(changes[1].as_ref().unwrap().title, "new, title");
    assert!(changes[2].is_none());
    done_tx.send(()).unwrap();
    server.join().unwrap();
}

#[test]
fn refresh_cannot_publish_a_focus_pair_split_across_a_recording_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let commands = UnixListener::bind(directory.path().join(".socket.sock")).unwrap();
    let events = UnixListener::bind(directory.path().join(".socket2.sock")).unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut old_events, _) = events.accept().unwrap();
        let (mut first, _) = commands.accept().unwrap();
        let mut command = [0; 14];
        first.read_exact(&mut command).unwrap();
        first
            .write_all(br#"{"address":"0xabc","class":"editor","title":"original"}"#)
            .unwrap();
        drop(first);
        old_events
            .write_all(b"activewindow>>private,hidden title\n")
            .unwrap();
        ready_tx.send(()).unwrap();
        let (mut refresh, _) = commands.accept().unwrap();
        refresh.read_exact(&mut command).unwrap();
        events.set_nonblocking(true).unwrap();
        let new_events = events.accept().ok();
        refresh
            .write_all(br#"{"address":"0xdef","class":"editor","title":"fresh public focus"}"#)
            .unwrap();
        drop(refresh);
        let _ = old_events.write_all(b"activewindowv2>>abc\n");
        ready_tx.send(()).unwrap();
        done_rx.recv().unwrap();
        drop(new_events);
    });
    let mut source = Hyprland::connect(directory.path()).unwrap();
    ready_rx.recv().unwrap();
    assert!(source.drain().unwrap().is_empty());
    assert_eq!(
        source.refresh().unwrap().unwrap().title,
        "fresh public focus"
    );
    ready_rx.recv().unwrap();
    let changes = source.drain().unwrap();
    done_tx.send(()).unwrap();
    server.join().unwrap();
    assert!(changes.is_empty(), "pre-boundary focus leaked: {changes:?}");
}
