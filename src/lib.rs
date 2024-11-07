use rsheet_lib::cell_expr::{CellArgument, CellExpr};
use rsheet_lib::cell_value::CellValue;
use rsheet_lib::command::{CellIdentifier, Command};
use rsheet_lib::connect::{
    Connection, Manager, ReadMessageResult, Reader, WriteMessageResult, Writer,
};
use rsheet_lib::replies::Reply;

use std::collections::HashMap;
use std::error::Error;

use log::info;

pub fn start_server<M>(mut manager: M) -> Result<(), Box<dyn Error>>
where
    M: Manager,
{
    // This initiates a single client connection, and reads and writes messages
    // indefinitely.
    let (mut recv, mut send) = match manager.accept_new_connection() {
        Connection::NewConnection { reader, writer } => (reader, writer),
        Connection::NoMoreConnections => {
            // There are no more new connections to accept.
            return Ok(());
        }
    };

    let mut data: HashMap<CellIdentifier, CellValue> = HashMap::new();
    loop {
        info!("Just got message");
        match recv.read_message() {
            ReadMessageResult::Message(msg) => {
                // rsheet_lib already contains a FromStr<Command> (i.e. parse::<Command>)
                // implementation for parsing the get and set commands. This is just a
                // demonstration of how to use msg.parse::<Command>, you may want/have to
                // change this code.
                let reply = match msg.parse::<Command>() {
                    Ok(command) => match command {
                        Command::Get { cell_identifier } => {
                            let id = identifier_to_string(cell_identifier);
                            if let Some(value) = data.get(&cell_identifier) {
                                Reply::Value(id, value.clone())
                            } else {
                                Reply::Value(id, CellValue::None)
                            }
                        }
                        Command::Set {
                            cell_identifier,
                            cell_expr,
                        } => {
                            let id = identifier_to_string(cell_identifier);
                            let expression = CellExpr::new(&cell_expr);
                            let mut test = HashMap::new();
                            let _ = data.iter().for_each(|(key, value)| {
                                test.insert(
                                    identifier_to_string(*key),
                                    CellArgument::Value(value.clone()),
                                );
                            });
                            if let Ok(res) = expression.evaluate(&test) {
                                data.insert(cell_identifier, res.clone());
                                Reply::Value(id, res)
                            } else {
                                Reply::Value(id, CellValue::None)
                            }
                        }
                    },
                    Err(e) => Reply::Error(e),
                };

                match send.write_message(reply) {
                    WriteMessageResult::Ok => {
                        // Message successfully sent, continue.
                    }
                    WriteMessageResult::ConnectionClosed => {
                        // The connection was closed. This is not an error, but
                        // should terminate this connection.
                        break;
                    }
                    WriteMessageResult::Err(e) => {
                        // An unexpected error was encountered.
                        return Err(Box::new(e));
                    }
                }
            }
            ReadMessageResult::ConnectionClosed => {
                // The connection was closed. This is not an error, but
                // should terminate this connection.
                break;
            }
            ReadMessageResult::Err(e) => {
                // An unexpected error was encountered.
                return Err(Box::new(e));
            }
        }
    }
    Ok(())
}

fn identifier_to_string(id: CellIdentifier) -> String {
    let col = rsheet_lib::cells::column_number_to_name(id.col);
    format!("{}{}", col, id.row + 1)
}
