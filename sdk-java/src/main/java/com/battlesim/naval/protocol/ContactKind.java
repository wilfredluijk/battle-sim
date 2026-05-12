package com.battlesim.naval.protocol;

public enum ContactKind {
    SHIP,
    SHELL,
    UNKNOWN;

    public static ContactKind fromWire(String s) {
        return switch (s) {
            case "ship" -> SHIP;
            case "shell" -> SHELL;
            default -> UNKNOWN;
        };
    }
}
