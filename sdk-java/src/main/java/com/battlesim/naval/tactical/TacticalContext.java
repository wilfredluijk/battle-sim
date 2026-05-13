package com.battlesim.naval.tactical;

import com.battlesim.naval.protocol.SelfState;
import com.battlesim.naval.protocol.ShipSpecs;
import com.battlesim.naval.protocol.WorldView;

/** All the state {@link TacticalBot#decide} should ever need. */
public record TacticalContext(
        WorldView view,
        SelfState me,
        ShipSpecs specs,
        Tracker tracker,
        ThreatList threats,
        float mapWidth,
        float mapHeight) {}
