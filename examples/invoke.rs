use std::{convert::TryInto, path::Path};

use ubus::{BlobMsg, UbusObject};

fn main() {
    let obj_path = "network.device";
    let method = "status";
    let args = "{\"name\":\"eth0\"}";

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
    .lookup(
        obj_path,
        |obj| {
            obj_json = serde_json::to_string_pretty(&obj).unwrap();
        },
    )
    .unwrap();
    let obj: UbusObject = serde_json::from_str(&obj_json).unwrap();
    let args = obj.args_from_json(method, args).unwrap();
    let mut json_str = String::new();
    connection
        .invoke(obj.id, method, &args, |bi| {
            json_str = "{\n".to_string();
            let mut first = true;
            for x in bi {
                if !first {
                    json_str += ",\n";
                }
                //json_str += &format!("{:?}", x);
                let msg: BlobMsg = x.try_into().unwrap();
                json_str += &format!("{}", msg);
                first = false;
            }
            json_str += "\n}";
        })
        .unwrap();
    println!("{}", json_str);
}
