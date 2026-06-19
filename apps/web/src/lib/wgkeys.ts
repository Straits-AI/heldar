// Client-side WireGuard keypair generation. Generating the keypair in the browser (and sending only the
// PUBLIC key to the box) means the peer private key never crosses the wire — the secure enrollment path.
// WireGuard uses Curve25519, which is exactly what nacl.box keypairs are.
import nacl from "tweetnacl";

/** The sentinel the server emits in the returned .conf's PrivateKey line when the client supplied its
 *  own public key. The client substitutes its locally-held private key in its place. Must match
 *  CLIENT_KEY_PLACEHOLDER in crates/heldar-kernel/src/services/wireguard.rs. */
export const CLIENT_KEY_PLACEHOLDER = "__REPLACE_WITH_DEVICE_PRIVATE_KEY__";

function toBase64(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s);
}

/** Generate a WireGuard (Curve25519) keypair in the browser. The private key stays on this device. */
export function generateWgKeypair(): { privateKey: string; publicKey: string } {
  const kp = nacl.box.keyPair();
  return { privateKey: toBase64(kp.secretKey), publicKey: toBase64(kp.publicKey) };
}

/** Splice the locally-held private key into the placeholder the server returned. */
export function fillPrivateKey(config: string, privateKey: string): string {
  return config.replace(CLIENT_KEY_PLACEHOLDER, privateKey);
}
