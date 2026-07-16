# hello-catalog

The smallest useful add-on: a browsable catalog with search and resolve, one
allowlisted API domain, and a single enum setting.

Try it against the simulator:

```sh
rvah validate examples/addons/hello-catalog/manifest.toml
rvah keygen -o /tmp/keys
rvah package examples/addons/hello-catalog -o /tmp/hello.rvah --key /tmp/keys/signing.key
rvah install /tmp/hello.rvah --pubkey /tmp/keys/verify.pub --data-dir /tmp/rvah-dev --approve
rvah browse com.example.hello-catalog --data-dir /tmp/rvah-dev
```

`module.wasm` is a placeholder entry name — the simulator drives the fixture
provider; a real build would ship a wasm module implementing ABI v0 (see
docs/developer-guide.md).
