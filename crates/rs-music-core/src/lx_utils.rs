use aes::{
    Aes128,
    cipher::{BlockEncryptMut, KeyInit, KeyIvInit, block_padding::Pkcs7},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use md5::{Digest, Md5};
use rand::RngCore;
use rquickjs::{Ctx, Exception, Function};

pub(super) fn install(ctx: &Ctx<'_>) -> rquickjs::Result<()> {
    let globals = ctx.globals();
    globals.set(
        "__md5",
        Function::new(ctx.clone(), |s: String| format!("{:x}", Md5::digest(s)))?,
    )?;
    globals.set(
        "__randomBytes",
        Function::new(ctx.clone(), |ctx: Ctx<'_>, size: u32| {
            if size > 2_097_152 {
                return Err(Exception::throw_range(&ctx, "randomBytes exceeds 2 MiB"));
            }
            let mut bytes = vec![0; size as usize];
            rand::rng().fill_bytes(&mut bytes);
            Ok(bytes)
        })?,
    )?;
    globals.set(
        "__fromText",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'_>, s: String, encoding: String| match encoding.as_str() {
                "utf8" | "utf-8" => Ok(s.into_bytes()),
                "base64" => STANDARD
                    .decode(s)
                    .map_err(|_| Exception::throw_type(&ctx, "Invalid base64")),
                "hex" => hex::decode(s).map_err(|_| Exception::throw_type(&ctx, "Invalid hex")),
                _ => Err(Exception::throw_type(&ctx, "Unsupported encoding")),
            },
        )?,
    )?;
    globals.set(
        "__toText",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'_>, bytes: Vec<u8>, encoding: String| match encoding.as_str() {
                "utf8" | "utf-8" => Ok(String::from_utf8_lossy(&bytes).into_owned()),
                "base64" => Ok(STANDARD.encode(bytes)),
                "hex" => Ok(hex::encode(bytes)),
                _ => Err(Exception::throw_type(&ctx, "Unsupported encoding")),
            },
        )?,
    )?;
    globals.set(
        "__aesEncrypt",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'_>, bytes: Vec<u8>, mode: String, key: Vec<u8>, iv: Vec<u8>| {
                match mode.as_str() {
                    "aes-128-ecb" => ecb::Encryptor::<Aes128>::new_from_slice(&key)
                        .map(|cipher| cipher.encrypt_padded_vec_mut::<Pkcs7>(&bytes)),
                    "aes-128-cbc" => cbc::Encryptor::<Aes128>::new_from_slices(&key, &iv)
                        .map(|cipher| cipher.encrypt_padded_vec_mut::<Pkcs7>(&bytes)),
                    _ => return Err(Exception::throw_type(&ctx, "Unsupported AES mode")),
                }
                .map_err(|_| Exception::throw_type(&ctx, "Invalid AES key or IV"))
            },
        )?,
    )?;
    Ok(())
}
