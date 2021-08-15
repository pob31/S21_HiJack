/*
  ==============================================================================

    OSCvalues.h
    Created: 13 Aug 2021 11:16:55pm
    Author:  Pierre-Olivier

  ==============================================================================
*/

#pragma once
#include <iostream>
#include <string>
#include <cmath>

class OSCfloat {

public:
    OSCfloat(std::string oscMethod, float min, float max, float value);
    ~OSCfloat();

    float clamp(float val);
    void updateOSCmethod(std::string oscMtd);
    
private:
    float min, max, value;
    std::string oscMethod;
};

class OSCwholeFloat {

public:
    OSCwholeFloat(std::string oscMethod, float min, float max, float value);
    ~OSCwholeFloat();

    float clamp(float val);
    void updateOSCmethod(std::string oscMtd);
    
private:
    float min, max, value;
    std::string oscMethod;

};

class OSCaction {

public:
    OSCaction(std::string oscMtd);
    ~OSCaction();

private:
    std::string oscMethod;

};