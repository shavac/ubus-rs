use serde_json::{to_string_pretty, Value};
use std::env;
use std::path::Path;

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut obj_path = "";
    let mut method = "";
    let mut data = "";
    if args.len() < 2 || args.len() > 4 {
        eprintln!("{} <object> <method> [arguments as json]", args[0]);
        return;
    } else if args.len() >= 3 {
        obj_path = &args[1];
        method = &args[2];
    }
    if args.len() == 4 {
        data = &args[3];
    }

    let socket = Path::new("/var/run/ubus/ubus.sock");

    let mut connection = match ubus::Connection::connect(&socket) {
        Ok(connection) => connection,
        Err(err) => {
            eprintln!("{}: Failed to open ubus socket. {}", socket.display(), err);
            return;
        }
    };
    let json = connection.call(obj_path, method, data).unwrap();
    let parsed: Value = serde_json::from_str(&json).unwrap();
    let pretty_json = to_string_pretty(&parsed).unwrap();
    println!("{}", pretty_json);
}
