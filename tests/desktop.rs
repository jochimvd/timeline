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
