import sys
import struct
import uuid
import json
import io 

class RpcException(Exception):
    def __init__(self, msg):
        super().__init__(f"RPC ERROR: {msg}")

class ProtocolError(Exception):
    def __init__(self, msg):
        super().__init__(f"PROTOCOL ERROR: {msg}")


def read_op_code_sync():
    return ord(sys.stdin.buffer.read(1))

class Scraper:
    def __init__(self, func, inp, out):
        self.scrape = func
        self.input = inp
        self.output = out


    def run(self):
        while True:
            opcode = self.read_byte()
            if opcode == 2:
                return
            elif opcode == 0:
                header, body = self.read_response()
                print("scraping", file=sys.stderr)
                self.scrape(self, header, body)
                print("scraped", file=sys.stderr)

                self.end_file()
            else:
                raise ProtocolError(f"unexpected opcode {opcode}")

    def read_byte(self):
        return ord(self.input.read(1))

    def read_response(self):
        header = self.read_with_len()
        body = bytearray()
        
        while True:
            length_bytes = self.input.read(8)
            length = struct.unpack("<Q", length_bytes)[0]

            if length == 0:
                break 

            body.extend(self.input.read(length))

        return json.loads(header.decode("utf8")), io.BytesIO(body)

    def read_with_len(self):
        length_bytes = self.input.read(8)
        length = struct.unpack("<Q", length_bytes)[0]
        data = self.input.read(length)
        return data

    def submit(self, url):
        self.output.write(struct.pack("<B", 0))
        self.write_str_with_len(url)
    
    def fetch(self): 
        self.output.write(struct.pack("<B", 1))
        self.write_str_with_len(url)
        
        op_code = self.read_byte()
        if op_code != 1:
            raise ProtocolError(f"unexpected op code f{op_code} - expected 1")
        
        is_error = self.read_byte() == 1
        if is_error:
            raise RpcException(self.read_with_len().decode("utf8"))
        else:
            return self.read_response()

    def end_file(self):
        self.output.write(struct.pack("<B", 2))
        self.output.flush()

    def write_str_with_len(self, data):
        data = data.encode()
        length = struct.pack("<H", len(data))
        self.output.write(length)
        self.output.write(data)
        self.output.flush()

def run(func):
    scraper = Scraper(func, sys.stdin.buffer, sys.stdout.buffer)
    scraper.run()