Public, disposable localhost test identity; never a deployment credential.
Both the acceptance fixture and the independent raw Quinn reproducer trust this
certificate explicitly. The private key is intentionally distributed with it.
Generated with OpenSSL using P-256, SAN DNS:localhost, CA:FALSE, digitalSignature.
The certificate is valid for 100 years from creation to avoid clock-dependent PVs.
