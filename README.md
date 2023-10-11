# Log3

Log3 is a `console.sol` log extraction tool for mined transactions. It is heavily relient on [Foundry](https://github.com/foundry-rs/foundry).
The purpose of creating this is to allow a more robust type of runtime logs while maintaining zero gas cost.

<p align="center">
<img src="https://github.com/spherex-xyz/log3/assets/20728006/4b043f2c-8a9c-45cb-abe5-3621dfee5d66" />
</p>

# CLI

## Build

```bash
cargo build -r
```

## Run

```bash
./target/release/log3 <ETHERSCAN_API_KEY> <CONTRACT_ADDRESS> <TX_HASH> <ENDPOINT>
```

for example:

```bash
./target/release/log3 <ETHERSCAN_API_KEY> 0x60FB45483d9E48dFC602f49E094024F8DA292416 0xa41a5e85874c305b2cdc1c0f529d2025ae41b5ed865452999ace0c385cc9fc4e <ENDPOINT>
```

Will output:

```
coinbase  0x1aD91ee08f21bE3dE0BA2ba6918E714dA6B45836
difficulty  79520372386923644452263703657155088832667823295608004009718642224436144452329
_nonces  6414
0x409d634b2f86a76bc9706c74fe5e9dc9081bd8076327ca22af1337b71846b819
2585
7846
```

# Lambda

All the following command should be run from the `lambda` directory

## Run

This will spawn a local server on port 9000

```bash
cargo lambda watch
```

Sample requests can be found in the [requests.http](./requests.http) file

## Build

```bash
cargo lambda build -r
```

## Deploy

```bash
cargo lambda deploy -r us-east-1 -v --enable-function-url --env-var "HOME=/mnt/data"  log3-lambda
```
