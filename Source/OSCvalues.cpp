/*
  ==============================================================================

    OSCvalues.cpp
    Created: 13 Aug 2021 11:16:55pm
    Author:  Pierre-Olivier

  ==============================================================================
*/

#include "OSCvalues.h"

// class OSCfloat


OSCfloat::OSCfloat(std::string oscMethod, float min, float max, float value) :
    oscMethod(oscMethod),
    min(min),
    max(max),
    value(value)
    {
        value = OSCfloat::clamp(value);
    }

OSCfloat::~OSCfloat()
{
}

void OSCfloat::updateOSCmethod(std::string oscMtd)
{
    oscMethod = oscMtd;
}

float OSCfloat::clamp(float val)
{
    if (val < OSCfloat::min) {
        return OSCfloat::min;
    }
    else if (val > OSCfloat::max) {
        return OSCfloat::max;
    }
    else {
        return val;
    }
}

// class OSCwholeFloat

OSCwholeFloat::OSCwholeFloat(std::string oscMethod, float min, float max, float value) :
    oscMethod(oscMethod),
    min(min),
    max(max),
    value(value)
    {
        value = OSCwholeFloat::clamp(value);
    }
    
OSCwholeFloat::~OSCwholeFloat()
{
}

float OSCwholeFloat::clamp(float val)
{
    if (val < OSCwholeFloat::min) {
        return OSCwholeFloat::min;
    }
    else if (val > OSCwholeFloat::max) {
        return OSCwholeFloat::max;
    }
    else {
        return std::floor(val);
    }
}

void OSCwholeFloat::updateOSCmethod(std::string oscMtd)
{
    oscMethod = oscMtd;
}

// class OSCaction

OSCaction::OSCaction(std::string oscMtd)
{
    OSCaction::oscMethod = oscMtd;
}

OSCaction::~OSCaction()
{
}