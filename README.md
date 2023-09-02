# Build

```bash
cargo lambda build -r
```

# Deploy

```bash
cargo lambda deploy -r us-east-1 -v --enable-function-url --env-var "HOME=/mnt/data"  log3-lambda
```
