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

**Region Pack**:
A downloadable road graph covering one region (initially one country), installed on the phone so the Routing Engine can compute Legs without connectivity. Several packs may be loaded together for cross-border Plans.
_Avoid_: Map download (that is map tiles), region, extract

**Map Pack**:
A downloadable set of map tiles covering one region, installed on the phone so the map can be drawn without connectivity. Distinct from a Region Pack, which holds the road graph the Routing Engine uses.
_Avoid_: Offline map, tile bundle, map download

**Reference Consumption**:
The single user-facing calibration number of a Vehicle Model: the energy per km the car uses at a steady 110 km/h in mild conditions. Adjusting it scales the Energy Model's predictions; it does not replace the Energy Model.
_Avoid_: Efficiency, Wh/km setting, consumption factor

**Charger Pack**:
A downloadable set of Chargers covering one region, built from open national datasets and installed on the phone so the planner can choose Charging Stops without connectivity. Refreshed independently of the Region Pack.
_Avoid_: Charger database, POI file, station list
