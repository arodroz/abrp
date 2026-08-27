# ABRP-native

A native iOS EV route planner replicating what is specific about ABRP (energy-aware routing with charging stops), built in Swift with Rust where performance demands it.

## Language

**Plan**:
The result of planning one journey: an ordered sequence of Legs and Charging Stops from origin to destination, with the predicted SoC curve.
_Avoid_: Route (that is a Leg's geometry), trip, itinerary

**Leg**:
A drive between two consecutive waypoints of a Plan (origin, Charging Stop or destination), with its road geometry, distance, duration and predicted energy use.
_Avoid_: Segment, section

**Charging Stop**:
A Charger chosen by the planner where the vehicle arrives at one SoC and leaves at a higher target SoC after a predicted charging duration.
_Avoid_: Charge, stop, station

**Charger**:
A physical charging location with one or more connectors, a maximum power and an operator. The data object, independent of any Plan.
_Avoid_: Station, POI, pole

**SoC**:
State of charge, the battery level as a percentage of usable capacity.
_Avoid_: Battery %, charge level, range

**Vehicle Model**:
The set of parameters describing one car's energy behaviour: usable capacity, mass, drag, rolling resistance, drivetrain efficiency, charging curve. The first Vehicle Model is the Hyundai Ioniq 5 (2022).
_Avoid_: Car profile, consumption model (that is the Energy Model)

**Energy Model**:
The function that predicts a Leg's energy use from a Vehicle Model plus speed profile, elevation, temperature and wind.
_Avoid_: Consumption model, physics model

**Charging Curve**:
The vehicle's maximum accepted charging power as a function of SoC.
_Avoid_: Charge speed, charging profile

**Routing Engine**:
The component that computes a Leg's road geometry, distance and speed profile from the road graph.
_Avoid_: Router, navigation, directions
