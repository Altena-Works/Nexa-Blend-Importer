# Nexa Blend Importer adapter

The adapter uses a dedicated menu identifier, title and worker executable so it can coexist with Nexa Blend Camera. It launches `nexa-blend-importer.exe --gui --language ja|en`, using MMD's public English-mode export when available and Japanese otherwise.

The worker opens a standard `.blend` picker, then Model, Camera and Animation options. Generated PMX/VMD files are loaded through MMD's standard operations; the adapter does not automatically load or partially publish an unsupported import. Native qualification is still pending. See the [project README](../../README.md).

## Acknowledgements

We thank MikuMikuDance (MMD) creator 樋口優, the developers and contributors of MMDPlugin, MMEffect, Blender, Bullet, Saba and GLM, asset and reference-material creators, and the MMD community.
