/// 7-bit encodings used inside SysEx payloads.
public enum SevenBit {
    /// Packs 8-bit bytes: each group of up to seven bytes is preceded by a
    /// byte holding their high bits, most significant first.
    public static func pack(_ bytes: [UInt8]) -> [UInt8] {
        var out: [UInt8] = []
        out.reserveCapacity(bytes.count + bytes.count / 7 + 1)
        var index = 0
        while index < bytes.count {
            let group = bytes[index..<min(index + 7, bytes.count)]
            var header: UInt8 = 0
            for (offset, byte) in group.enumerated() where byte & 0x80 != 0 {
                header |= 0x40 >> UInt8(offset)
            }
            out.append(header)
            out.append(contentsOf: group.map { $0 & 0x7F })
            index += 7
        }
        return out
    }

    public static func unpack(_ packed: [UInt8]) -> [UInt8] {
        var out: [UInt8] = []
        out.reserveCapacity(packed.count)
        var header: UInt8 = 0
        for (index, byte) in packed.enumerated() {
            let position = index % 8
            if position == 0 {
                header = byte
            } else {
                out.append(((header << UInt8(position)) & 0x80) | (byte & 0x7F))
            }
        }
        return out
    }

    /// File names travel as UTF-16 code units, low byte first, then packed.
    public static func packName(_ name: String) -> [UInt8] {
        pack(name.utf16.flatMap { [UInt8($0 & 0xFF), UInt8($0 >> 8)] })
    }

    public static func unpackName(_ packed: [UInt8]) -> String {
        let bytes = unpack(packed)
        var units: [UInt16] = []
        units.reserveCapacity(bytes.count / 2)
        var index = 0
        while index + 1 < bytes.count {
            let unit = UInt16(bytes[index]) | UInt16(bytes[index + 1]) << 8
            if unit == 0 { break }
            units.append(unit)
            index += 2
        }
        return String(decoding: units, as: UTF16.self)
    }

    /// 14-bit value as two bytes, low seven bits first.
    public static func split14(_ value: Int) -> [UInt8] {
        [UInt8(value & 0x7F), UInt8((value >> 7) & 0x7F)]
    }

    public static func join14(low: UInt8, high: UInt8) -> Int {
        Int(low & 0x7F) | Int(high & 0x7F) << 7
    }

    /// 64-bit value from ten bytes of seven bits each, least significant first.
    public static func join64(_ bytes: ArraySlice<UInt8>) -> UInt64 {
        var value: UInt64 = 0
        for (index, byte) in bytes.prefix(10).enumerated() {
            value |= UInt64(byte & 0x7F) << UInt64(7 * index)
        }
        return value
    }

    public static func split64(_ value: UInt64) -> [UInt8] {
        (0..<10).map { UInt8((value >> UInt64(7 * $0)) & 0x7F) }
    }
}
