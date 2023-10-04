mod analysis;
mod block;
mod opcodes;
mod printer;

use clap::{Arg, Command};
use evmil::analysis::{insert_havocs,trace};
use evmil::bytecode::{Assembly, Instruction, StructuredSection};
use evmil::bytecode::Instruction::*;
use evmil::util::{FromHexString,ToHexString};

use analysis::{State};
use printer::*;

type PreconditionFn = fn(&Instruction);

fn gen_proof(bytes: &[u8], preconditions: PreconditionFn, blocksize: u16) {
    print_preamble(&bytes);    
    // Disassemble bytes into instructions
    let mut contract = Assembly::from_legacy_bytes(bytes);
    // Infer havoc instructions
    contract = infer_havoc_insns(contract);
    //
    let mut id = 0;
    for s in &contract {
        match s {
            StructuredSection::Code(insns) => {
                // Compute analysis results
                let init : State = State::new();
                // Run the abstract trace
                let states : Vec<Vec<State>> = trace(&insns,init);
                // Print out the code
                print_code_section(id, insns, &states, preconditions, blocksize)
            }
            StructuredSection::Data(bytes) => {
                // For now.
                println!("// {}",bytes.to_hex_string());
            }
        }
        id = id + 1;
    }
}

fn infer_havoc_insns(mut asm: Assembly) -> Assembly {
    // This could probably be more efficient :)
    let sections = asm.iter_mut().map(|section| {
        match section {
            StructuredSection::Code(ref mut insns) => {    
                let ninsns = insert_havocs(insns.clone());
	        StructuredSection::Code(ninsns)
            }
            _ => section.clone()
        }
    }).collect();
    // 
    Assembly::new(sections)
}

/// Add assertions to check against overflow / underflow in generated
/// bytecode.
fn overflow_checks(insn: &Instruction) {
    match insn {
        ADD => println!("\tassert (st.Peek(0) + st.Peek(1)) <= (MAX_U256 as u256);"),
        MUL => println!("\tassert (st.Peek(0) * st.Peek(1)) <= (MAX_U256 as u256);"),
        SUB => println!("\tassert st.Peek(1) <= st.Peek(0);"),
        _ => {
            // do nothing
        }
    }    
}

// This is a hack script for now.
fn main() {
    //let args: Vec<String> = env::args().collect();
    let matches = Command::new("devmpg")
        .about("DafnyEvm Proof Generation Tool")
        .arg(Arg::new("args"))
        .arg(Arg::new("overflow").long("overflows"))
        .arg(Arg::new("blocksize")
             .long("blocksize")
             .value_name("SIZE")
             .value_parser(clap::value_parser!(u16))
             .default_value("65535"))
        .get_matches();
    // Extract arguments
    let overflows = matches.is_present("overflow");
    let blocksize : &u16 = matches.get_one("blocksize").unwrap();
    let args: Vec<_> = matches.get_many::<String>("args").unwrap().collect();
    // Done
    for arg in args {
        // Parse hex string into bytes
        let bytes = arg.from_hex_string().unwrap();
        gen_proof(&bytes, overflow_checks, *blocksize);
    }
}
