use openssl::{
    asn1::Asn1Time,
    bn::BigNum,
    hash::MessageDigest,
    pkey::{PKey, Private},
    x509::{extension, X509NameBuilder, X509},
};

pub(in super::super) fn test_root(key: &PKey<Private>) -> X509 {
    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_text("CN", "role-grant-test-root")
        .unwrap();
    let name = name.build();
    let mut root = X509::builder().unwrap();
    root.set_version(2).unwrap();
    root.set_serial_number(&BigNum::from_u32(1).unwrap().to_asn1_integer().unwrap())
        .unwrap();
    root.set_subject_name(&name).unwrap();
    root.set_issuer_name(&name).unwrap();
    root.set_pubkey(key).unwrap();
    root.set_not_before(&Asn1Time::days_from_now(0).unwrap())
        .unwrap();
    root.set_not_after(&Asn1Time::days_from_now(1).unwrap())
        .unwrap();
    root.append_extension(
        extension::BasicConstraints::new()
            .critical()
            .ca()
            .build()
            .unwrap(),
    )
    .unwrap();
    root.append_extension(
        extension::KeyUsage::new()
            .critical()
            .key_cert_sign()
            .build()
            .unwrap(),
    )
    .unwrap();
    root.sign(key, MessageDigest::sha256()).unwrap();
    root.build()
}
