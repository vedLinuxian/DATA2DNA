# Security Policy

## Reporting Vulnerabilities

If you discover a security vulnerability, please email:
vedcimit@gmail.com

Do NOT open a public GitHub issue for security vulnerabilities.

## Scope

DATA2DNA is a data encoding/decoding tool. Security considerations include:
- Data integrity (hash verification)
- No network data transmission (all processing is local)
- The web interface binds to localhost by default

## Note on Cryptographic Use

DATA2DNA uses CRC-32 for oligo integrity checking. CRC-32 is NOT
a cryptographic hash and provides no security guarantees against
adversarial tampering. For security-sensitive archives, hash your
data with SHA-256 before encoding.
