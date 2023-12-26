use std::{env, path::Path};

use ubus::BlobMsgPayload;

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut obj_path = "";
    if args.len() > 1 {
        obj_path = args[1].as_str();
    }
    let socket = Path::new("/var/run/ubus/ubus.sock");

    let mut connection = match ubus::Connection::connect(&socket) {
        Ok(connection) => connection,
        Err(err) => {
            eprintln!("{}: Failed to open ubus socket. {}", socket.display(), err);
            return;
        }
    };
    connection
        .lookup(
            obj_path,
            |obj| {
                println!("{:?}", obj);
            },
            |sig| {
                println!("  {}:{:?}", sig.name, sig.args);
            },
        )
        .unwrap();
}
