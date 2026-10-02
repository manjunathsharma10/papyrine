"""TEST-ONLY oracle: signs PDFs and validates signatures with pyHanko.

    pyhanko_oracle.py sign   <in.pdf> <out.pdf> <workdir>
    pyhanko_oracle.py verify <pdf> <workdir>

pyHanko is installed into a venv under target/ by the test harness and is never shipped.
`verify` prints a JSON list with one object per embedded signature.
"""

import datetime
import json
import os
import sys


def make_identity(workdir):
    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import rsa
    from cryptography.x509.oid import NameOID

    key_path = os.path.join(workdir, "key.pem")
    cert_path = os.path.join(workdir, "cert.pem")
    if os.path.exists(key_path) and os.path.exists(cert_path):
        return key_path, cert_path
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "Papyrine Test Signer")])
    now = datetime.datetime.now(datetime.timezone.utc)
    cert = (
        x509.CertificateBuilder()
        .subject_name(name)
        .issuer_name(name)
        .public_key(key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(now - datetime.timedelta(days=1))
        .not_valid_after(now + datetime.timedelta(days=30))
        .add_extension(x509.BasicConstraints(ca=True, path_length=None), critical=True)
        .sign(key, hashes.SHA256())
    )
    with open(key_path, "wb") as f:
        f.write(
            key.private_bytes(
                serialization.Encoding.PEM,
                serialization.PrivateFormat.PKCS8,
                serialization.NoEncryption(),
            )
        )
    with open(cert_path, "wb") as f:
        f.write(cert.public_bytes(serialization.Encoding.PEM))
    return key_path, cert_path


def sign(inp, outp, workdir):
    from pyhanko.pdf_utils.incremental_writer import IncrementalPdfFileWriter
    from pyhanko.sign import fields, signers

    key, cert = make_identity(workdir)
    signer = signers.SimpleSigner.load(key, cert)
    with open(inp, "rb") as f:
        w = IncrementalPdfFileWriter(f)
        fields.append_signature_field(w, fields.SigFieldSpec("Sig1"))
        out = signers.sign_pdf(
            w, signers.PdfSignatureMetadata(field_name="Sig1"), signer=signer
        )
        with open(outp, "wb") as g:
            g.write(out.getbuffer())


def verify(path, workdir):
    from pyhanko.keys import load_cert_from_pemder
    from pyhanko.pdf_utils.reader import PdfFileReader
    from pyhanko.sign.validation import validate_pdf_signature
    from pyhanko_certvalidator import ValidationContext

    _, cert_path = make_identity(workdir)
    vc = ValidationContext(trust_roots=[load_cert_from_pemder(cert_path)])
    out = []
    with open(path, "rb") as f:
        r = PdfFileReader(f)
        for sig in r.embedded_signatures:
            st = validate_pdf_signature(sig, signer_validation_context=vc)
            out.append(
                {
                    "field": sig.field_name,
                    "intact": bool(st.intact),
                    "valid": bool(st.valid),
                    "trusted": bool(st.trusted),
                    "coverage": str(st.coverage),
                    "modification_level": str(st.modification_level),
                }
            )
    print(json.dumps(out))


if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "sign":
        sign(*sys.argv[2:5])
    elif cmd == "verify":
        try:
            verify(*sys.argv[2:4])
        except Exception as e:  # report instead of crashing: a broken file is a result
            print(json.dumps([{"error": f"{type(e).__name__}: {e}"}]))
    else:
        sys.exit("usage: sign|verify")
