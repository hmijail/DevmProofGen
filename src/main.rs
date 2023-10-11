mod analysis;
mod block;
mod opcodes;
mod printer;

use std::fs;
use std::fs::File;
use std::io::{BufWriter,Write};
use std::collections::HashMap;
use std::error::Error;
use clap::{Arg, Command};
use evmil::analysis::{insert_havocs,trace};
use evmil::bytecode::{Assemble, Assembly, Instruction, StructuredSection};
use evmil::bytecode::Instruction::*;
use evmil::util::{FromHexString,ToHexString};

use analysis::{State};
use block::{Block,BlockSequence};
use printer::*;


fn main() -> Result<(), Box<dyn Error>> {
    //let args: Vec<String> = env::args().collect();
    let matches = Command::new("devmpg")
        .about("DafnyEvm Proof Generation Tool")
        .arg(Arg::new("overflow").long("overflows"))
        .arg(Arg::new("blocksize")
             .long("blocksize")
             .value_name("SIZE")
             .value_parser(clap::value_parser!(usize))
             .default_value("65535"))
        .arg(Arg::new("split").long("split").value_name("json-file"))
        .arg(Arg::new("target").required(true))        
        .get_matches();
    // Extract arguments
    let overflows = matches.is_present("overflow");
    let blocksize : &usize = matches.get_one("blocksize").unwrap();
    let target = matches.get_one::<String>("target").unwrap();
    // Read from asm file
    let bytes = fs::read(target)?;    
    // Generate the proof
    gen_proof(&bytes, overflow_checks, *blocksize);
    // ===============================================================
    // New style
    // ===============================================================
    let mut roots = HashMap::new();
    let prefix = default_prefix(target);
    roots.insert((0,0),"main".to_string());
    // Disassemble bytes into instructions
    let mut contract = Assembly::from_legacy_bytes(&bytes);
    // Infer havoc instructions
    contract = infer_havoc_insns(contract);
    // Deconstruct into sequences
    let blkseqs = deconstruct(&contract,*blocksize);
    // Group subsequences
    let groups = group(roots,&blkseqs);
    //
    write_headers(&prefix,&contract);
    // Write files
    write_groups(&prefix,groups);
    // Done
    Ok(())
}

fn default_prefix(name: &str) -> String {
    name.replace(".","_")
}

struct BlockGroup {
    id: usize,
    name: String,
    blocks: Vec<Block>
}

// Given an assembly, deconstruct it into a set of blocks of a given
// maximum size.
fn deconstruct(contract: &Assembly, blocksize: usize) -> Vec<BlockSequence> {
    let mut blocks = Vec::new();
    //
    for s in contract {
        match s {
            StructuredSection::Code(insns) => {
                blocks.push(BlockSequence::from_insns(blocksize,insns));
            }
            StructuredSection::Data(bytes) => {
                // Nothing (for now)
            }
        }
    }
    //
    blocks
}

// Given a sequence of blocks, generate a set of block groups.
fn group(roots: HashMap<(usize,usize),String>, blocks: &[BlockSequence]) -> Vec<BlockGroup> {
    let mut groups = Vec::new();
    //
    for (i,blk) in blocks.iter().enumerate() {
        groups.extend(split(&roots,i,blk));
    }
    //
    groups
}

/// Split a given sequence of blocks (in the same code segment) upto
/// into one or more groups.
fn split(roots: &HashMap<(usize,usize),String>, id: usize, blocks: &BlockSequence) -> Vec<BlockGroup> {
    let name = roots.get(&(id,0x00)).unwrap().clone();
    // HACK FOR NOW
    let grp = BlockGroup{id,name,blocks: blocks.clone().to_vec()};
    // Done?
    vec![grp]
}

/// Convert each block group into a sequence of one or more files
/// using a given prefix.
fn write_groups(prefix: &str, groups: Vec<BlockGroup>) -> Result<(), Box<dyn Error>> {
    for g in groups {
        let filename = format!("{prefix}_{}_{}.dfy",g.id,g.name);
        let header = format!("{prefix}_{}_header.dfy",g.id);        
        println!("Writing {filename}");
        let mut f = BufWriter::new(File::create(filename)?);
        writeln!(f,"include \"../evm-dafny/src/dafny/evm.dfy\"");
        writeln!(f,"include \"../evm-dafny/src/dafny/core/code.dfy\"");        
        writeln!(f,"include \"{header}\"");                
        writeln!(f,"");
        writeln!(f,"module {} {{",g.name);
        writeln!(f,"\timport opened Opcode");
        writeln!(f,"\timport opened Code");
        writeln!(f,"\timport opened Memory");
        writeln!(f,"\timport opened Bytecode");
        writeln!(f,"\timport opened Header");        
        writeln!(f,"");                
        // Construct block printer
        let mut printer = DafnyPrinter::new(g.id);
        //
        for blk in g.blocks {
            printer.print_block(&blk);
        }
        // Write the file
        write!(f,"{}",printer.to_string());
        writeln!(f,"}}");
    }
    Ok(())
}
 
/// Write out header files for all bytecode sections.
fn write_headers(prefix: &str, contract: &Assembly) -> Result<(), Box<dyn Error>> {
    for (i,s) in contract.iter().enumerate() {
        match s {
            StructuredSection::Code(insns) => {
                let filename = format!("{prefix}_{}_header.dfy",i);
                println!("Writing {filename}");
                let mut f = BufWriter::new(File::create(filename)?);
                writeln!(f,"include \"../evm-dafny/src/dafny/evm.dfy\"")?;
                writeln!(f,"")?;
                writeln!(f,"module Header {{")?;
                writeln!(f,"\timport opened Int");
                writeln!(f,"");                
                write_bytecode(&mut f, insns, i);
                writeln!(f,"}}")?;
            }
            StructuredSection::Data(bytes) => {
                // Nothing (for now)
            }
        }
    }
    Ok(())
}

/// Write out the contract bytecode as an array of bytes.
fn write_bytecode<T:Write>(f: &mut T, insns: &[Instruction], id: usize) {
    // Convert instructions into bytes
    let mut bytes = insns.assemble();   
    //
    write!(f,"\tconst BYTECODE_{id} : seq<u8> := [");
    //
    for i in 0..bytes.len() {
        if i%8 == 0 { write!(f,"\n\t\t"); }
        write!(f,"{:#02x}", bytes[i]);
        if (i + 1) != bytes.len() {
            write!(f,", ");
        }
    }
    writeln!(f,"\n\t];");
}

// ===================================================================

type PreconditionFn = fn(&Instruction);

fn gen_proof(bytes: &[u8], preconditions: PreconditionFn, blocksize: usize) {
    // Print necessary dafny preamble
    print_preamble(bytes);    
    // Disassemble bytes into instructions
    let mut contract = Assembly::from_legacy_bytes(bytes);
    // Infer havoc instructions
    contract = infer_havoc_insns(contract);
    //
    let mut id = 0;
    for s in &contract {
        match s {
            StructuredSection::Code(insns) => {
                // Construct block printer
                let mut printer = DafnyPrinter::new(id);
                // Print raw bytecode
                printer.print_bytecode(insns);
                // Build initial block sequence
                let blocks = BlockSequence::from_insns(blocksize,insns);                
                //
                for blk in blocks.iter() {
                    printer.print_block(blk);
                }
                //
                println!("{}",printer.to_string());
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

pub fn print_preamble(bytes: &[u8]) {
    println!("include \"evm-dafny/src/dafny/evm.dfy\"");
    println!("include \"evm-dafny/src/dafny/evms/berlin.dfy\"");
    println!("import opened Opcode");
    println!("import opened Memory");
    println!("import opened Bytecode");
    println!("type u8 = Int.u8");
    println!("type u160 = Int.u160");
    println!();
    println!("method external_call(sender: u160, st: EvmState.ExecutingState) returns (r:EvmState.TerminatedState)");
    println!("ensures r.RETURNS? ==> r.world.Exists(sender) {{");
    println!("\t return EvmState.ERROR(EvmState.INSUFFICIENT_GAS); // dummy");
    println!("}}");
    println!();
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
