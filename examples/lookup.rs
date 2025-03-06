use std::{env, path::Path};

use ubus::UbusObject;

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
    let mut obj_json = String::new();
    connection
        .lookup(obj_path, |obj| {
            obj_json = serde_json::to_string_pretty(&obj).unwrap();
        })
        .unwrap();
    let obj: UbusObject = serde_json::from_str(&obj_json).unwrap();
    println!("{:?}", obj);
}
