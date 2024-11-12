use rsheet_lib::cell_expr::{CellArgument, CellExpr};
use rsheet_lib::cell_value::CellValue;
use rsheet_lib::command::{CellIdentifier, Command};
use rsheet_lib::connect::{
    Connection, Manager, ReadMessageResult, Reader, WriteMessageResult, Writer,
};
use rsheet_lib::replies::Reply;

use std::collections::{HashMap, HashSet};
use std::error::Error;

use log::info;

#[derive(Clone, Debug)]
struct Value {
    value: Result<CellValue, Reply>,
    dep: HashSet<CellIdentifier>,
    expression: String,
}

// This should be all working except concurrency isnt done
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
    let mut data: HashMap<CellIdentifier, Value> = HashMap::new();
    loop {
        // dbg!(data.clone());
        info!("Just got message");
        match recv.read_message() {
            ReadMessageResult::Message(msg) => {
                // rsheet_lib already contains a FromStr<Command> (i.e. parse::<Command>)
                // implementation for parsing the get and set commands. This is just a
                // demonstration of how to use msg.parse::<Command>, you may want/have to
                // change this code.
                let reply = match msg.trim().parse::<Command>() {
                    Ok(command) => match command {
                        Command::Get { cell_identifier } => {
                            let id = identifier_to_string(cell_identifier);
                            if let Some(value) = data.get(&cell_identifier) {
                                match &value.value {
                                    Ok(val) => Reply::Value(id, val.clone()),
                                    Err(error) => error.clone(),
                                }
                            } else {
                                Reply::Value(id, CellValue::None)
                            }
                        }
                        Command::Set {
                            cell_identifier,
                            cell_expr,
                        } => {
                            set_expression(cell_identifier, cell_expr, &mut data);
                            continue;
                        }
                    },
                    Err(_) => Reply::Error(String::from("Invalid key provided")),
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

fn set_expression(
    cell_identifier: CellIdentifier,
    cell_expr: String,
    data: &mut HashMap<CellIdentifier, Value>,
) {
    let expression = CellExpr::new(&cell_expr);
    let mut variables_set = HashSet::new();
    let mut vars: HashMap<String, CellArgument> = HashMap::new();
    expression
        .find_variable_names()
        .into_iter()
        .for_each(|var| {
            variables_set.insert(var);
        });
    // put into seperate function
    variables_set.iter().for_each(|var| {
        if var.contains("_") {
            let mut ends = Vec::new();
            var.split("_").into_iter().for_each(|elem| ends.push(elem));
            if ends.len() == 2 {
                let start = ends[0].parse::<CellIdentifier>().unwrap();
                let end = ends[1].parse::<CellIdentifier>().unwrap();
                // dbg!(start);
                // dbg!(end);
                if start.col == end.col && start.row != end.row {
                    let mut args_vec: Vec<CellValue> = Vec::new();
                    (start.row..end.row + 1).for_each(|row| {
                        let res = data.get_key_value(&CellIdentifier {
                            col: start.col,
                            row,
                        });
                        match res {
                            Some((_, value)) => {
                                if let Ok(val) = &value.value {
                                    args_vec.push(val.clone());
                                }
                            }
                            None => args_vec.push(CellValue::None),
                        }
                    });
                    vars.insert(var.clone(), CellArgument::Vector(args_vec));
                    // same col
                } else if start.col != end.col && start.row == end.row {
                    let mut args_vec: Vec<CellValue> = Vec::new();
                    (start.col..end.col + 1).for_each(|col| {
                        let res = data.get_key_value(&CellIdentifier {
                            row: start.row,
                            col,
                        });
                        match res {
                            Some((_, value)) => {
                                if let Ok(val) = &value.value {
                                    args_vec.push(val.clone())
                                }
                            }
                            None => args_vec.push(CellValue::None),
                        }
                    });
                    vars.insert(var.clone(), CellArgument::Vector(args_vec));
                    // same row
                } else {
                    let mut args_matrix: Vec<Vec<CellValue>> = Vec::new();
                    (start.col..end.col + 1).for_each(|col| {
                        let mut args_vec: Vec<CellValue> = Vec::new();
                        (start.row..end.row + 1).for_each(|row| {
                            let res = data.get_key_value(&CellIdentifier { row, col });
                            match res {
                                Some((_, value)) => {
                                    if let Ok(val) = &value.value {
                                        args_vec.push(val.clone())
                                    }
                                }
                                None => args_vec.push(CellValue::None),
                            }
                        });
                        args_matrix.push(args_vec);
                    });
                    vars.insert(var.clone(), CellArgument::Matrix(args_matrix));
                    // matrix
                }
            }
        } else {
            let id = var.parse::<CellIdentifier>().unwrap();
            let res = data.get_key_value(&id);
            match res {
                Some((_, value)) => {
                    if let Ok(val) = &value.value {
                        vars.insert(var.clone(), CellArgument::Value(val.clone()));
                    }
                }
                None => {
                    vars.insert(var.clone(), CellArgument::Value(CellValue::None));
                }
            }
        }
    });
    // put into seperate function
    let mut dep: HashSet<_> = HashSet::new();
    variables_set.iter().for_each(|var| {
        // HAVE TO TEST COL AND COMPLETE ROW AND MATRIX
        if var.contains("_") {
            // need to split and push all elements in
            let mut ends = Vec::new();
            var.split("_").into_iter().for_each(|elem| {
                ends.push(elem);
            });
            if ends.len() == 2 {
                let start = ends[0].parse::<CellIdentifier>().unwrap();
                let end = ends[1].parse::<CellIdentifier>().unwrap();
                // have to push all dependencies into set
                if start.col == end.col && start.row != end.row {
                    // same col
                    (start.row..end.row + 1).into_iter().for_each(|row| {
                        dep.insert(CellIdentifier {
                            col: start.col,
                            row,
                        });
                    });
                } else if start.col != end.col && start.row == end.row {
                    // same row
                    (start.col..end.col + 1).into_iter().for_each(|col| {
                        dep.insert(CellIdentifier {
                            col,
                            row: start.row,
                        });
                    });
                } else {
                    (start.row..end.row + 1).into_iter().for_each(|row| {
                        (start.col..end.col + 1).into_iter().for_each(|col| {
                            dep.insert(CellIdentifier { col, row });
                        });
                    });
                    // neither same row or col
                }
            }
        } else {
            dep.insert(var.parse::<CellIdentifier>().unwrap());
        }
    });
    // set A1 sum(B1_B10)
    // dbg!(vars.clone());
    // dbg!(dep.clone());
    // dbg!(cell_identifier);
    // Maybe can make it not enter this if all dependencies aren't in var
    match expression.evaluate(&vars) {
        Ok(res) => {
            data.insert(
                cell_identifier,
                Value {
                    value: Ok(res),
                    dep,
                    expression: cell_expr,
                },
            );
            data.clone()
                .iter()
                .filter(|(_, value)| value.dep.contains(&cell_identifier))
                .for_each(|(id, val)| {
                    set_expression(*id, val.expression.clone(), data);
                });
        }
        Err(_) => {
            data.insert(
                cell_identifier,
                Value {
                    value: Err(Reply::Error(String::from(
                        "Error: Variable depends on value Error",
                    ))),
                    dep,
                    expression: cell_expr,
                },
            );
        }
    }
}

fn identifier_to_string(id: CellIdentifier) -> String {
    let col = rsheet_lib::cells::column_number_to_name(id.col);
    format!("{}{}", col, id.row + 1)
}
