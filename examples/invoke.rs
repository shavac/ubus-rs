use std::path::Path;

fn main() {
    let obj_path = "network.device";
    let method = "status";
    let args = r#"{"name":"eth0"}"#;

    let socket = Path::new("/var/run/ubus/ubus.sock");

    let mut connection = ubus::Connection::connect(socket)
        .map_err(|err| {
            eprintln!("{}: Failed to open ubus socket. {}", socket.display(), err);
            err
        })
        .unwrap();
    let mut obj_id = 0;
    let mut encoded_args = Vec::new();
    connection
        .lookup(obj_path, |obj| {
            obj_id = obj.id;
            encoded_args = obj.args_from_json(method, args).unwrap();
        })
        .unwrap();
    let mut json_str = String::new();
    connection
        .invoke(obj_id, method, &encoded_args, |bi| {
            json_str = serde_json::to_string_pretty(&bi.as_table()).unwrap();
        })
        .unwrap();
    println!("{}", json_str);
}
