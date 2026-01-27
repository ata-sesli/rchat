# <img src="rchat-logo.svg" height="30" alt="RChat Logo" /> RChat

> **Decentralized, Peer-to-Peer Local Messaging.**
<p align="center">
  <video src="https://github.com/user-attachments/assets/773ef4b0-6813-45da-a266-b6b56b7e4140" width="100%" autoplay loop muted playsinline></video>
</p>

RChat is a local-first, serverless communication tool built for secure and private messaging. It automatically discovers peers using **Hybrid Discovery** (mDNS + GitHub Gists) and enables direct sharing of text, images, documents, and videos. All group communications are secured using **Hierarchical Key Sharing (HKS)**, ensuring efficient and encrypted payload distribution.

### Tech Stack

- **Core**: [Rust](https://www.rust-lang.org/) & [Tauri](https://tauri.app/)
- **Frontend**: [Svelte](https://svelte.dev/) & [TailwindCSS](https://tailwindcss.com/)
- **Networking**: [libp2p](https://libp2p.io/) (Gossipsub & Direct P2P)
- **Security**: HKS (Hierarchical Key Sharing) & Ed25519/X25519
- **Discovery**: mDNS & GitHub Gists (via `octocrab`)
- **Storage**: SQLite

For a visual showcase, visit: [ata-sesli.github.io/rchat](https://ata-sesli.github.io/rchat/)

