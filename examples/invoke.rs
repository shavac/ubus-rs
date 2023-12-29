use std::{convert::TryInto, path::Path};

use ubus::{BlobMsg, BlobMsgPayload};

fn main() {
    let obj_path = "network.device";
    let method = "status";
    let args = Some(vec![BlobMsg {
        name: "name",
        data: BlobMsgPayload::String("eth0"),
    }]);

    let socket = Path::new("/var/run/ubus/ubus.sock");

    let mut connection = match ubus::Connection::connect(&socket) {
        Ok(connection) => connection,
        Err(err) => {
            eprintln!("{}: Failed to open ubus socket. {}", socket.display(), err);
            return;
        }
    };
    let obj_id = connection.lookup_id(obj_path).unwrap();
    connection
        .invoke(obj_id, method, args, |bi| {
            let mut json_output = "{\n".to_string();
            let mut first = true;
            for x in bi {
                if !first {
                    json_output += ",\n";
                }
                //json_output += &format!("{:?}", x);
                let msg: BlobMsg = x.try_into().unwrap();
                json_output += &format!("{}", msg);
                first = false;
            }
            json_output += "\n}";
            println!("{}", json_output);
        })
        .unwrap();
}
