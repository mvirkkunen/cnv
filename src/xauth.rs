use std::io::{ErrorKind, Read, Result};

#[derive(Clone, Debug)]
pub struct Entry {
    pub family: u16,
    pub address: Vec<u8>,
    pub number: Vec<u8>,
    pub name: Vec<u8>,
    pub data: Vec<u8>,
}

pub fn parse(r: &mut impl Read) -> Result<Vec<Entry>> {
    let mut result = vec![];
    loop {
        let family = match read_u16(r) {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        };

        result.push(Entry {
            family,
            address: read_string(r)?,
            number: read_string(r)?,
            name: read_string(r)?,
            data: read_string(r)?,
        });
    }

    Ok(result)
}

fn read_u16(r: &mut impl Read) -> Result<u16> {
    let mut buf = [0u8; 2];
    r.read_exact(&mut buf)?;
    Ok(u16::from_be_bytes(buf))
}

fn read_string(r: &mut impl Read) -> Result<Vec<u8>> {
    let mut buf = vec![0; read_u16(r)?.into()];
    r.read_exact(buf.as_mut_slice())?;
    Ok(buf)
}
