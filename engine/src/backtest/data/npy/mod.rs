use std::{
    fs::File,
    io::{Error, ErrorKind, Read, Write},
};

use crate::{
    backtest::data::{Data, DataPtr, POD, npy::parser::Value},
    utils::CACHE_LINE_SIZE,
};

mod parser;

pub trait NpyDTyped: POD {
    fn descr() -> DType;
}

pub type DType = Vec<Field>;

#[derive(PartialEq, Eq, Debug)]
pub struct NpyHeader {
    pub descr: DType,
    pub fortran_order: bool,
    pub shape: Vec<usize>,
}

#[derive(PartialEq, Eq, Debug)]
pub struct Field {
    pub name: String,
    pub ty: String,
}

impl NpyHeader {
    pub fn descr(&self) -> String {
        self.descr
            .iter()
            .map(|Field { name, ty }| format!("('{name}', '{ty}'), "))
            .fold("[".to_string(), |out, next| out + &next)
            + "]"
    }

    pub fn fortran_order(&self) -> String {
        if self.fortran_order {
            "True".into()
        } else {
            "False".into()
        }
    }

    pub fn shape(&self) -> String {
        self.shape
            .iter()
            .map(|len| format!("{len}, "))
            .fold("(".to_string(), |out, next| out + &next)
            + ")"
    }

    pub fn from_header(header: &str) -> std::io::Result<Self> {
        let (_, header) = parser::parse::<(&str, nom::error::ErrorKind)>(header)
            .map_err(|error| Error::new(ErrorKind::InvalidData, error.to_string()))?;
        let dict = header.get_dict()?;
        let mut descr = Vec::new();
        let mut fortran_order = false;
        let mut shape = Vec::new();
        for (key, value) in dict {
            match key.as_str() {
                "descr" => {
                    for item in value.get_list()? {
                        let tuple = item.get_list()?;
                        if tuple.len() != 2 {
                            return Err(Error::new(
                                ErrorKind::InvalidData,
                                "dtype entry must contain 2 items",
                            ));
                        }
                        match (&tuple[0], &tuple[1]) {
                            (Value::String(name), Value::String(dtype)) => descr.push(Field {
                                name: name.clone(),
                                ty: dtype.clone(),
                            }),
                            _ => {
                                return Err(Error::new(
                                    ErrorKind::InvalidData,
                                    "invalid dtype entry",
                                ));
                            }
                        }
                    }
                }
                "fortran_order" => fortran_order = value.get_bool()?,
                "shape" => {
                    for number in value.get_list()? {
                        shape.push(number.get_integer()?);
                    }
                }
                _ => {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "unexpected numpy header key",
                    ));
                }
            }
        }
        Ok(Self {
            descr,
            fortran_order,
            shape,
        })
    }

    fn to_string_padding(&self) -> String {
        let mut header = format!(
            "{{'descr': {}, 'fortran_order': {}, 'shape': {}}}",
            self.descr(),
            self.fortran_order(),
            self.shape()
        );
        let header_len = 10 + header.len() + 1;
        if !header_len.is_multiple_of(64) {
            header.push_str(&" ".repeat((header_len / 64 + 1) * 64 - header_len));
        }
        header.push('\n');
        header
    }
}

fn check_field_consistency(expected: &DType, found: &DType) -> std::io::Result<()> {
    if expected.len() != found.len() {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "numpy dtype field count mismatch",
        ));
    }
    for (expected, found) in expected.iter().zip(found.iter()) {
        if expected.ty != found.ty {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "dtype mismatch: expected {}:{}, found {}:{}",
                    expected.name, expected.ty, found.name, found.ty
                ),
            ));
        }
    }
    Ok(())
}

pub fn read_npy<R: Read, D: NpyDTyped + Clone>(
    reader: &mut R,
    size: usize,
) -> std::io::Result<Data<D>> {
    const PREAMBLE_LEN: usize = 10;

    if size < PREAMBLE_LEN {
        return Err(Error::new(
            ErrorKind::UnexpectedEof,
            "numpy file is shorter than its preamble",
        ));
    }

    let mut buf = DataPtr::new(size);
    let mut read_size = 0;
    while read_size < size {
        let n = reader.read(&mut buf[read_size..])?;
        if n == 0 {
            return Err(Error::new(
                ErrorKind::UnexpectedEof,
                "unexpected end of numpy file",
            ));
        }
        read_size += n;
    }

    if buf[0..6].to_vec() != b"\x93NUMPY" {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "must start with \\x93NUMPY",
        ));
    }
    if buf[6..8].to_vec() != b"\x01\x00" {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "only numpy version 1.0 is supported",
        ));
    }
    let header_len = u16::from_le_bytes(buf[8..10].try_into().unwrap()) as usize;
    let header_end = PREAMBLE_LEN.checked_add(header_len).ok_or_else(|| {
        Error::new(
            ErrorKind::InvalidData,
            "numpy header length exceeds addressable memory",
        )
    })?;
    if header_end > size {
        return Err(Error::new(
            ErrorKind::UnexpectedEof,
            "numpy header extends past end of file",
        ));
    }
    let header = String::from_utf8(buf[PREAMBLE_LEN..header_end].to_vec())
        .map_err(|error| Error::new(ErrorKind::InvalidData, error.to_string()))?;
    let header = NpyHeader::from_header(&header)?;
    if header.fortran_order {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "fortran order is unsupported",
        ));
    }
    if D::descr() != header.descr {
        check_field_consistency(&D::descr(), &header.descr)?;
    }
    if header.shape.len() != 1 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "only one-dimensional numpy arrays are supported",
        ));
    }
    if !header_end.is_multiple_of(CACHE_LINE_SIZE) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "numpy data is not cache-line aligned",
        ));
    }
    Ok(unsafe { Data::from_data_ptr(buf, header_end) })
}

pub fn read_npy_file<D: NpyDTyped + Clone>(filepath: &str) -> std::io::Result<Data<D>> {
    let mut file = File::open(filepath)?;
    let size = file.metadata()?.len() as usize;
    read_npy(&mut file, size)
}

pub fn read_npz_file<D: NpyDTyped + Clone>(filepath: &str, name: &str) -> std::io::Result<Data<D>> {
    let mut archive = zip::ZipArchive::new(File::open(filepath)?)?;
    let mut file = archive.by_name(&format!("{name}.npy"))?;
    let size = file.size() as usize;
    read_npy(&mut file, size)
}

pub fn write_npy<W: Write, T: NpyDTyped>(write: &mut W, data: &[T]) -> std::io::Result<()> {
    let header = NpyHeader {
        descr: T::descr(),
        fortran_order: false,
        shape: vec![data.len()],
    };
    write.write_all(b"\x93NUMPY\x01\x00")?;
    let header = header.to_string_padding();
    write.write_all(&(header.len() as u16).to_le_bytes())?;
    write.write_all(header.as_bytes())?;
    write.write_all(as_bytes(data))?;
    Ok(())
}

fn as_bytes<T>(values: &[T]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), std::mem::size_of_val(values))
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, ErrorKind};

    use crate::types::Event;

    use super::read_npy;

    #[test]
    fn read_npy_rejects_a_truncated_preamble() {
        let bytes = b"\x93NUMPY\x01\x00";
        let mut reader = Cursor::new(bytes);

        let error = read_npy::<_, Event>(&mut reader, bytes.len()).unwrap_err();

        assert_eq!(error.kind(), ErrorKind::UnexpectedEof);
    }

    #[test]
    fn read_npy_rejects_a_header_length_past_end_of_file() {
        let mut bytes = b"\x93NUMPY\x01\x00".to_vec();
        bytes.extend_from_slice(&u16::MAX.to_le_bytes());
        bytes.resize(64, b' ');
        let mut reader = Cursor::new(&bytes);

        let error = read_npy::<_, Event>(&mut reader, bytes.len()).unwrap_err();

        assert_eq!(error.kind(), ErrorKind::UnexpectedEof);
    }
}
