use rsheet_lib::cell_expr::{CellArgument, CellExpr};
use rsheet_lib::cell_value::CellValue;
use rsheet_lib::command::{CellIdentifier, Command};
use rsheet_lib::connect::{
    Connection, Manager, ReadMessageResult, Reader, ReaderWriter, WriteMessageResult, Writer,
};
use rsheet_lib::replies::Reply;

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use log::info;

// This Fails
// a: set A1 3
// b: set A2 sleep_then(2000, 1)
// b: get A1

#[derive(Clone, Debug)]
struct Value {
    value: Result<CellValue, Reply>,
    dep: HashSet<CellIdentifier>,
    expression: String,
    time: SystemTime,
}

// This should be all working except concurrency isnt done
pub fn start_server<M>(mut manager: M) -> Result<(), Box<dyn Error + Send + Sync>>
where
    M: Manager + Send + 'static,
{
    let data: Arc<RwLock<HashMap<CellIdentifier, Value>>> = Arc::new(RwLock::new(HashMap::new()));
    let mut threads = Vec::new();
    while let Connection::NewConnection { reader, writer } = manager.accept_new_connection() {
        let data = data.clone();
        threads.push(std::thread::spawn(
            move || -> Result<(), Box<dyn Error + Send + Sync>> {
                create_new_connection::<M>(reader, writer, data)
            },
        ));
    }

    for handle in threads {
        handle.join().unwrap()?;
    }
    Ok(())
}

fn create_new_connection<M>(
    mut recv: <<M as Manager>::ReaderWriter as ReaderWriter>::Reader,
    mut send: <<M as Manager>::ReaderWriter as ReaderWriter>::Writer,
    data: Arc<RwLock<HashMap<CellIdentifier, Value>>>,
) -> Result<(), Box<dyn Error + Send + Sync>>
where
    M: Manager + Send + 'static,
{
    loop {
        let data = data.clone();
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
                            let id = identifier_to_string(&cell_identifier);
                            let d = data.read().unwrap();
                            if let Some(value) = d.get(&cell_identifier) {
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
                            set_expression(cell_identifier, cell_expr.clone(), data.clone());
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

/// function that takes
///     - the CellIdentifier used in the command e.g. the 'A1' in set A1 1
///     - the expression string used in the command e.g. the 'sum(A1_B2) in set C1 sum(A1_B2)
///     - the data is the entire dataset in the spreadsheet
/// this function executes the set command
fn set_expression(
    cell_identifier: CellIdentifier,
    cell_expr: String,
    data: Arc<RwLock<HashMap<CellIdentifier, Value>>>,
) {
    // Need to look into if i need to make another thread to do updates
    let (expression, variables_set) = get_variables_set(&cell_expr);
    let d = data.read().unwrap();
    let vars: HashMap<String, CellArgument> = get_vars(&variables_set, &d);
    drop(d);
    let dep: HashSet<_> = get_dependencies(&variables_set);

    match expression.evaluate(&vars) {
        Ok(res) => {
            let d = data.read().unwrap();
            if d.contains_key(&cell_identifier)
                && d.get(&cell_identifier).unwrap().time > SystemTime::now()
            {
                return;
            }
            drop(d);
            let mut d = data.write().unwrap();
            d.insert(
                cell_identifier,
                Value {
                    value: Ok(res),
                    dep,
                    expression: cell_expr,
                    time: SystemTime::now(),
                },
            );
            drop(d);
            let mut effected_values = Vec::new();
            data.read()
                .unwrap()
                .iter()
                .filter(|(_, value)| value.dep.contains(&cell_identifier))
                .for_each(|(id, val)| {
                    effected_values.push((*id, val.clone()));
                });
            effected_values.into_iter().for_each(|(id, val)| {
                set_expression(id, val.expression, data.clone());
            });
        }
        Err(_) => {
            let mut d = data.write().unwrap();
            d.insert(
                cell_identifier,
                Value {
                    value: Err(Reply::Error(String::from(
                        "Error: Variable depends on value Error",
                    ))),
                    dep,
                    expression: cell_expr,
                    time: SystemTime::now(),
                },
            );
            drop(d);
        }
    }
    drop(data);
}
/// function that takes:
///     - an expr which is the expression string from the user entered command.
///       e.g. the 'sum(A1_B2)' in set C1 sum(A1_B2)
/// returns a tuple of the CellExpr generated from the string,
///         and all the variables used in the expression.
fn get_variables_set(expr: &str) -> (CellExpr, HashSet<String>) {
    let mut variables_set = HashSet::new();
    let expression = CellExpr::new(expr);
    expression
        .find_variable_names()
        .into_iter()
        .for_each(|var| {
            variables_set.insert(var);
        });
    (expression, variables_set)
}

/// function to convert CellIdentifier into a string
/// e.g. from CellIdentifier {
///     col: 0,
///     row: 0,
/// } to "A1"
fn identifier_to_string(id: &CellIdentifier) -> String {
    let col = rsheet_lib::cells::column_number_to_name(id.col);
    format!("{}{}", col, id.row + 1)
}

/// function that takes in variables_set which is the set of variables present in an expression
/// these have been extracted using the function find_variable_names from CellExpr in rsheet_lib.
/// also takes in data which is the memory of the spreadsheet.
/// returns a map of the arguments names and their associated value.
/// arguments name instead of cell id as name could be value, scalar or matrix
/// designed to be the argument in the function CellExpr::evaluate()
fn get_vars(
    variables_set: &HashSet<String>,
    data: &HashMap<CellIdentifier, Value>,
) -> HashMap<String, CellArgument> {
    let mut vars = HashMap::new();
    variables_set.iter().for_each(|var| {
        if var.contains("_") {
            let mut ends = Vec::new();
            var.split("_").for_each(|elem| ends.push(elem));
            // variables_set ensures that each String will be valid.
            // So either it has an _ and it has 2 elements or it doesnt and it have 1
            if ends.len() == 2 {
                let start = ends[0].parse::<CellIdentifier>().unwrap();
                let end = ends[1].parse::<CellIdentifier>().unwrap();
                if start.col == end.col && start.row != end.row {
                    // same col
                    vars.insert(
                        var.clone(),
                        CellArgument::Vector(get_var_column_values(start, end, data)),
                    );
                } else if start.col != end.col && start.row == end.row {
                    // same row
                    vars.insert(
                        var.clone(),
                        CellArgument::Vector(get_var_row_values(start, end, data)),
                    );
                } else {
                    vars.insert(
                        var.clone(),
                        CellArgument::Matrix(get_var_matrix_values(start, end, data)),
                    );
                    // matrix
                }
            }
        } else {
            vars.insert(
                var.clone(),
                CellArgument::Value(get_single_value(var, data)),
            );
        }
    });
    vars
}

/// function that takes in variables_set which is the set of variables present in an expression
/// these have been extracted using the function find_variable_names from CellExpr in rsheet_lib.
/// returns all the CellIdentifiers that the expression is dependent on
fn get_dependencies(variables_set: &HashSet<String>) -> HashSet<CellIdentifier> {
    let mut dep = HashSet::new();
    variables_set.iter().for_each(|var| {
        if var.contains("_") {
            let mut ends = Vec::new();
            var.split("_").for_each(|elem| {
                ends.push(elem);
            });
            // variables_set ensures that each String will be valid.
            // So either it has an _ and it has 2 elements or it doesnt and it have 1
            if ends.len() == 2 {
                let start = ends[0].parse::<CellIdentifier>().unwrap();
                let end = ends[1].parse::<CellIdentifier>().unwrap();
                // have to insert all dependencies into set
                if start.col == end.col && start.row != end.row {
                    // same col
                    (start.row..=end.row).for_each(|row| {
                        dep.insert(CellIdentifier {
                            col: start.col,
                            row,
                        });
                    });
                } else if start.col != end.col && start.row == end.row {
                    // same row
                    (start.col..=end.col).for_each(|col| {
                        dep.insert(CellIdentifier {
                            col,
                            row: start.row,
                        });
                    });
                } else {
                    // neither same row or col
                    (start.row..=end.row).for_each(|row| {
                        (start.col..=end.col).for_each(|col| {
                            dep.insert(CellIdentifier { col, row });
                        });
                    });
                }
            }
        } else {
            dep.insert(var.parse::<CellIdentifier>().unwrap());
        }
    });
    dep
}

/// function that takes:
///     - the start and end CellIdentifier in an expression of which the columns are equal
///     - the entire dataset of the spreadsheet
/// returns all of the values in the correct order in a Vec
/// e.g returns that values of A1, A2 and A3 in a Vec for the expression set B1 sum(A1_A3)
fn get_var_column_values(
    start: CellIdentifier,
    end: CellIdentifier,
    data: &HashMap<CellIdentifier, Value>,
) -> Vec<CellValue> {
    (start.row..=end.row)
        .map(|row| {
            data.get(&CellIdentifier {
                col: start.col,
                row,
            })
            .and_then(|value| value.value.clone().ok())
            .unwrap_or(CellValue::None)
        })
        .collect()
}

/// function that takes:
///     - the start and end CellIdentifier in an expression of which the rows are equal
///     - the entire dataset of the spreadsheet
/// returns all of the values in the correct order in a Vec
/// e.g returns that values of A1, B1 and C1 in a Vec for the expression set A2 sum(A1_C13)
fn get_var_row_values(
    start: CellIdentifier,
    end: CellIdentifier,
    data: &HashMap<CellIdentifier, Value>,
) -> Vec<CellValue> {
    (start.col..=end.col)
        .map(|col| {
            data.get(&CellIdentifier {
                col,
                row: start.row,
            })
            .and_then(|value| value.value.clone().ok())
            .unwrap_or(CellValue::None)
        })
        .collect()
}

/// function that takes:
///     - the start and end CellIdentifier in an expression of which neither the columns nor the rows are equal
///     - the entire dataset of the spreadsheet
/// returns all of the values in the correct order in a Vec of a Vec where the inner vec is the values of the rows
/// e.g returns that values of A1, A2 and A3 in a Vec for the expression set B1 sum(A1_A3)
fn get_var_matrix_values(
    start: CellIdentifier,
    end: CellIdentifier,
    data: &HashMap<CellIdentifier, Value>,
) -> Vec<Vec<CellValue>> {
    (start.col..=end.col)
        .map(|col| {
            (start.row..=end.row)
                .map(|row| {
                    data.get(&CellIdentifier { col, row })
                        .and_then(|value| value.value.clone().ok())
                        .unwrap_or(CellValue::None)
                })
                .collect()
        })
        .collect()
}

/// function that takes:
///     - the Cell in the form of a &str
///     - the entire dataset of the spreadsheet
/// returns all of the values in the correct order in a Vec of a Vec where the inner vec is the values of the rows
/// e.g returns that values of A1, A2 and A3 in a Vec for the expression set B1 sum(A1_A3)
fn get_single_value(var: &str, data: &HashMap<CellIdentifier, Value>) -> CellValue {
    var.parse::<CellIdentifier>()
        .ok()
        .and_then(|id| data.get(&id).and_then(|value| value.value.clone().ok()))
        .unwrap_or(CellValue::None)
}
