
export type FakeResolver = {
  address: string;
  txt(name: string, values: string[]): void;
  asked: string[];
  stop(): void;
};

export async function fakeResolver(): Promise<FakeResolver> {
  const zone = new Map<string, string[]>();
  const asked: string[] = [];
  const socket = await Bun.udpSocket({
    hostname: "127.0.0.1",
    port: 0,
    socket: {
      data(socket, packet, port, address) {
        const query = Buffer.from(packet);
        const labels: string[] = [];
        let at = 12;
        while (at < query.length && query[at] !== 0) {
          labels.push(query.subarray(at + 1, at + 1 + query[at]).toString("ascii"));
          at += query[at] + 1;
        }
        const questionEnd = at + 5;
        const name = labels.join(".").toLowerCase();
        asked.push(name);
        const values = zone.get(name);
        const answers = values?.map((value) => {
          const text = Buffer.from(value, "utf8");
          const record = Buffer.alloc(12 + 1 + text.length);
          record.set([0xc0, 0x0c, 0, 16, 0, 1, 0, 0, 0, 60], 0);
          record.writeUInt16BE(text.length + 1, 10);
          record[12] = text.length;
          text.copy(record, 13);
          return record;
        }) ?? [];
        const header = Buffer.alloc(12);
        query.copy(header, 0, 0, 2);
        header[2] = 0x81;
        header[3] = values ? 0x80 : 0x83;
        header.writeUInt16BE(1, 4);
        header.writeUInt16BE(answers.length, 6);
        socket.send(Buffer.concat([header, query.subarray(12, questionEnd), ...answers]), port, address);
      },
    },
  });
  return {
    address: `127.0.0.1:${socket.port}`,
    txt(name, values) { if (values.length) zone.set(name.toLowerCase(), values); else zone.delete(name.toLowerCase()); },
    asked,
    stop() { socket.close(); },
  };
}
