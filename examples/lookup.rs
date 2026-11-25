use std::{env, path::Path};

fn main() {
    let obj_path = env::args().nth(1).unwrap_or_default();
    let socket = Path::new("/var/run/ubus/ubus.sock");

    let mut connection = match ubus::Connection::connect(socket) {
        Ok(connection) => connection,
        Err(err) => {
            eprintln!("{}: Failed to open ubus socket. {}", socket.display(), err);
            return;
        }
    };
    connection
        .lookup(&obj_path, |obj| {
            println!("{:?}", obj);
        })
        .unwrap();
}
