package com.battlesim.naval.protocol;

public enum SensorMode {
    ACTIVE("active"),
    PASSIVE("passive");

    private final String wire;

    SensorMode(String wire) {
        this.wire = wire;
    }

    public String wire() {
        return wire;
    }
}
