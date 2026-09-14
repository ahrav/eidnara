import { createPublicKey, generateKeyPairSync, sign } from "node:crypto";

export function selfSignedCertificate(dnsName: string): { key: string; cert: string } {
    const { privateKey } = generateKeyPairSync("ec", { namedCurve: "prime256v1" });
    const subjectPublicKeyInfo = createPublicKey(privateKey).export({
        type: "spki",
        format: "der",
    });
    const ecdsaWithSha256 = sequence(oid("1.2.840.10045.4.3.2"));
    const name = sequence(set(sequence(oid("2.5.4.3"), utf8String(dnsName))));
    const now = Date.now();
    const basicConstraints = sequence(
        oid("2.5.29.19"),
        boolean(true),
        octetString(sequence(boolean(true))),
    );
    const subjectAltName = sequence(
        oid("2.5.29.17"),
        octetString(sequence(der(0x82, Buffer.from(dnsName, "ascii")))),
    );
    const tbsCertificate = sequence(
        der(0xa0, integer(2)),
        integer(1),
        ecdsaWithSha256,
        name,
        sequence(utcTime(new Date(now - 60_000)), utcTime(new Date(now + 3_600_000))),
        name,
        subjectPublicKeyInfo,
        der(0xa3, sequence(basicConstraints, subjectAltName)),
    );
    const signature = sign("sha256", tbsCertificate, { key: privateKey, dsaEncoding: "der" });
    const certificate = sequence(
        tbsCertificate,
        ecdsaWithSha256,
        der(0x03, Buffer.from([0x00]), signature),
    );
    return {
        key: privateKey.export({ type: "pkcs8", format: "pem" }) as string,
        cert: pem("CERTIFICATE", certificate),
    };
}

function der(tag: number, ...parts: Buffer[]): Buffer {
    const body = Buffer.concat(parts);
    if (body.length >= 0x10000) throw new RangeError("DER element exceeds 65535 bytes");
    const length =
        body.length < 0x80
            ? Buffer.from([body.length])
            : body.length < 0x100
              ? Buffer.from([0x81, body.length])
              : Buffer.from([0x82, body.length >> 8, body.length & 0xff]);
    return Buffer.concat([Buffer.from([tag]), length, body]);
}

const sequence = (...parts: Buffer[]) => der(0x30, ...parts);
const set = (...parts: Buffer[]) => der(0x31, ...parts);
const octetString = (body: Buffer) => der(0x04, body);
const utf8String = (text: string) => der(0x0c, Buffer.from(text, "utf8"));
const boolean = (value: boolean) => der(0x01, Buffer.from([value ? 0xff : 0x00]));
const integer = (value: number) => der(0x02, Buffer.from([value]));

function oid(dotted: string): Buffer {
    const arcs = dotted.split(".").map(Number);
    const bytes: number[] = [arcs[0] * 40 + arcs[1]];
    for (const arc of arcs.slice(2)) {
        const chunk: number[] = [arc & 0x7f];
        for (let rest = arc >> 7; rest > 0; rest >>= 7) chunk.unshift((rest & 0x7f) | 0x80);
        bytes.push(...chunk);
    }
    return der(0x06, Buffer.from(bytes));
}

/** UTCTime `YYMMDDHHMMSSZ`; RFC 5280 requires this encoding for dates through 2049. */
function utcTime(date: Date): Buffer {
    const text = date.toISOString().replace(/[-:T]/g, "").slice(2, 14);
    return der(0x17, Buffer.from(`${text}Z`, "ascii"));
}

function pem(label: string, body: Buffer): string {
    const lines = body.toString("base64").match(/.{1,64}/g) ?? [];
    return `-----BEGIN ${label}-----\n${lines.join("\n")}\n-----END ${label}-----\n`;
}
