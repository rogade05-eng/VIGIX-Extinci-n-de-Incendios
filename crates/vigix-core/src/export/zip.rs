//! Contenedor ZIP **sin compresión** (método 0, "stored") con CRC-32.
//!
//! Es lo único que necesitan XLSX y los paquetes de exportación. Es **determinista**: marca de
//! fecha fija (1980-01-01), nombres UTF-8 y orden de inserción, de modo que mismas entradas =>
//! mismos bytes (y misma huella SHA-256).

use crate::hash::crc32;

struct Entry {
    name: String,
    crc: u32,
    size: u32,
    offset: u32,
}

pub struct ZipWriter {
    buf: Vec<u8>,
    entries: Vec<Entry>,
}

impl Default for ZipWriter {
    fn default() -> Self {
        Self::new()
    }
}

fn le16(b: &mut Vec<u8>, v: u16) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn le32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}

const DOS_TIME: u16 = 0;
const DOS_DATE: u16 = (1 << 5) | 1; // 1980-01-01

impl ZipWriter {
    pub fn new() -> ZipWriter {
        ZipWriter { buf: Vec::new(), entries: Vec::new() }
    }

    /// Añade un archivo. El nombre se sanea: sin `..`, sin `\`, sin ruta absoluta.
    pub fn add(&mut self, name: &str, data: &[u8]) {
        let clean: String =
            name.replace('\\', "/").split('/').filter(|s| !s.is_empty() && *s != "." && *s != "..").collect::<Vec<_>>().join("/");
        let crc = crc32(data);
        let offset = self.buf.len() as u32;
        le32(&mut self.buf, 0x0403_4b50);
        le16(&mut self.buf, 20); // versión necesaria
        le16(&mut self.buf, 0x0800); // nombres UTF-8
        le16(&mut self.buf, 0); // método: stored
        le16(&mut self.buf, DOS_TIME);
        le16(&mut self.buf, DOS_DATE);
        le32(&mut self.buf, crc);
        le32(&mut self.buf, data.len() as u32);
        le32(&mut self.buf, data.len() as u32);
        le16(&mut self.buf, clean.len() as u16);
        le16(&mut self.buf, 0);
        self.buf.extend_from_slice(clean.as_bytes());
        self.buf.extend_from_slice(data);
        self.entries.push(Entry { name: clean, crc, size: data.len() as u32, offset });
    }

    pub fn finish(mut self) -> Vec<u8> {
        let cd_start = self.buf.len() as u32;
        let entries = std::mem::take(&mut self.entries);
        for e in &entries {
            le32(&mut self.buf, 0x0201_4b50);
            le16(&mut self.buf, 20); // versión que lo creó
            le16(&mut self.buf, 20); // versión necesaria
            le16(&mut self.buf, 0x0800);
            le16(&mut self.buf, 0);
            le16(&mut self.buf, DOS_TIME);
            le16(&mut self.buf, DOS_DATE);
            le32(&mut self.buf, e.crc);
            le32(&mut self.buf, e.size);
            le32(&mut self.buf, e.size);
            le16(&mut self.buf, e.name.len() as u16);
            le16(&mut self.buf, 0); // extra
            le16(&mut self.buf, 0); // comentario
            le16(&mut self.buf, 0); // disco
            le16(&mut self.buf, 0); // atributos internos
            le32(&mut self.buf, 0); // atributos externos
            le32(&mut self.buf, e.offset);
            self.buf.extend_from_slice(e.name.as_bytes());
        }
        let cd_size = self.buf.len() as u32 - cd_start;
        le32(&mut self.buf, 0x0605_4b50);
        le16(&mut self.buf, 0);
        le16(&mut self.buf, 0);
        le16(&mut self.buf, entries.len() as u16);
        le16(&mut self.buf, entries.len() as u16);
        le32(&mut self.buf, cd_size);
        le32(&mut self.buf, cd_start);
        le16(&mut self.buf, 0);
        self.buf
    }
}

/// Lector mínimo de ZIP "stored" generado por este módulo (para pruebas y verificación).
pub fn read_stored(data: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    // EOCD al final (sin comentario)
    if data.len() < 22 {
        return None;
    }
    let e = data.len() - 22;
    if data[e..e + 4] != [0x50, 0x4b, 0x05, 0x06] {
        return None;
    }
    let n = u16::from_le_bytes([data[e + 10], data[e + 11]]) as usize;
    let mut off = u32::from_le_bytes([data[e + 16], data[e + 17], data[e + 18], data[e + 19]]) as usize;
    let mut out = Vec::new();
    for _ in 0..n {
        if data.get(off..off + 4)? != [0x50, 0x4b, 0x01, 0x02] {
            return None;
        }
        let method = u16::from_le_bytes([data[off + 10], data[off + 11]]);
        let crc = u32::from_le_bytes([data[off + 16], data[off + 17], data[off + 18], data[off + 19]]);
        let size = u32::from_le_bytes([data[off + 24], data[off + 25], data[off + 26], data[off + 27]]) as usize;
        let nlen = u16::from_le_bytes([data[off + 28], data[off + 29]]) as usize;
        let xlen = u16::from_le_bytes([data[off + 30], data[off + 31]]) as usize;
        let clen = u16::from_le_bytes([data[off + 32], data[off + 33]]) as usize;
        let lho = u32::from_le_bytes([data[off + 42], data[off + 43], data[off + 44], data[off + 45]]) as usize;
        let name = String::from_utf8(data.get(off + 46..off + 46 + nlen)?.to_vec()).ok()?;
        if method != 0 {
            return None;
        }
        // cabecera local
        let lnlen = u16::from_le_bytes([data[lho + 26], data[lho + 27]]) as usize;
        let lxlen = u16::from_le_bytes([data[lho + 28], data[lho + 29]]) as usize;
        let start = lho + 30 + lnlen + lxlen;
        let body = data.get(start..start + size)?.to_vec();
        if crc32(&body) != crc {
            return None;
        }
        out.push((name, body));
        off += 46 + nlen + xlen + clen;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ida_y_vuelta_y_determinismo() {
        let build = || {
            let mut z = ZipWriter::new();
            z.add("a.txt", b"hola");
            z.add("dir/b.bin", &[0u8, 1, 2, 255]);
            z.add("vacio", b"");
            z.add("ñandú.txt", "acentos".as_bytes());
            z.finish()
        };
        let a = build();
        assert_eq!(a, build(), "mismos datos => mismos bytes");
        let files = read_stored(&a).unwrap();
        assert_eq!(files.len(), 4);
        assert_eq!(files[0], ("a.txt".to_string(), b"hola".to_vec()));
        assert_eq!(files[1].0, "dir/b.bin");
        assert_eq!(files[2].1.len(), 0);
        assert_eq!(files[3].0, "ñandú.txt");
    }

    #[test]
    fn nombres_maliciosos_se_sanean() {
        let mut z = ZipWriter::new();
        z.add("../../etc/passwd", b"x");
        z.add("/abs/ruta.txt", b"y");
        z.add("a\\b\\c.txt", b"z");
        z.add("./x/./y.txt", b"w");
        let names: Vec<String> = read_stored(&z.finish()).unwrap().into_iter().map(|f| f.0).collect();
        assert_eq!(names, vec!["etc/passwd", "abs/ruta.txt", "a/b/c.txt", "x/y.txt"]);
    }

    #[test]
    fn deteccion_de_corrupcion() {
        let mut z = ZipWriter::new();
        z.add("a.txt", b"contenido importante");
        let mut bytes = z.finish();
        bytes[40] ^= 0xFF; // alterar un byte del cuerpo
        assert!(read_stored(&bytes).is_none(), "el CRC debe detectar la alteración");
        assert!(read_stored(b"no es un zip").is_none());
        assert!(read_stored(&[]).is_none());
    }
}
